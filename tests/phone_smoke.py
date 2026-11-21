#!/usr/bin/env python3
"""Opt-in live USB tests. Does not change phone networking or unrelated ADB forwards.

Build with `python3 scripts/build.py --dev` first. Uses the connected phone's
Internet to fetch public test pages; downloads no private/user data.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import http.client
import json
from pathlib import Path
import queue
import shutil
import signal
import socket
import ssl
import struct
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[1]


def exact(stream, count):
    result = b""
    while len(result) < count:
        part = stream.recv(count - len(result))
        if not part:
            raise RuntimeError("Unexpected EOF")
        result += part
    return result


def socks(port, host, remote_port, command=1, expected=0):
    stream = socket.create_connection(("127.0.0.1", port), timeout=20)
    stream.sendall(b"\x05\x01\x00")
    assert exact(stream, 2) == b"\x05\x00"
    domain = host.encode("ascii")
    stream.sendall(bytes([5, command, 0, 3, len(domain)]) + domain + struct.pack("!H", remote_port))
    response = exact(stream, 10)
    assert response[0] == 5 and response[1] == expected, response
    return stream


def https(port, host, path="/", body=None):
    with socks(port, host, 443) as raw:
        with ssl.create_default_context().wrap_socket(raw, server_hostname=host) as stream:
            payload = b"" if body is None else body
            method = "GET" if body is None else "POST"
            request = (
                f"{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n"
                f"Content-Length: {len(payload)}\r\nContent-Type: text/plain\r\n\r\n"
            ).encode() + payload
            stream.sendall(request)
            response = http.client.HTTPResponse(stream)
            response.begin()
            assert response.status == 200, (host, response.status)
            return response.read()


def forward_list(adb, device):
    output = subprocess.check_output([adb, "-s", device, "forward", "--list"], text=True)
    return set(tuple(line.split()) for line in output.splitlines() if line.strip())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adb", default=shutil.which("adb"))
    parser.add_argument("--device")
    parser.add_argument("--upload", action="store_true", help="Also send a 64 KiB generated payload to a public echo service")
    parser.add_argument("--upload-host", default="postman-echo.com", help="HTTPS service providing /post with a JSON data field")
    args = parser.parse_args()
    devices = subprocess.check_output([args.adb, "devices"], text=True)
    serials = [line.split()[0] for line in devices.splitlines()[1:] if len(line.split()) >= 2 and line.split()[1] == "device"]
    device = args.device or (serials[0] if len(serials) == 1 else None)
    assert device, "Select a USB device using --device SERIAL"
    before = forward_list(args.adb, device)
    process = subprocess.Popen([
        str(ROOT / "target/debug/transbox"), "--runtime", str(ROOT / ".payload/dev"),
        "--adb", args.adb, "--device", device, "--proxy-only", "--duration", "120",
    ], stdout=subprocess.PIPE, text=True, cwd=ROOT)
    stdout = process.stdout
    assert stdout is not None, "Launcher stdout must be piped"
    output: queue.Queue[str] = queue.Queue()

    def read_output():
        for line in stdout:
            output.put(line)

    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()
    owned_forward = None
    try:
        line = output.get(timeout=25)
        assert line.startswith("TRANSBOX_PROXY socks5h://127.0.0.1:"), line
        port = int(line.rsplit(":", 1)[1])
        added = forward_list(args.adb, device) - before
        assert len(added) == 1, added
        owned_forward = added.pop()
        assert b"Example Domain" in https(port, "example.com")
        print("PASS: TLS page fetched through phone-side DNS and TCP", flush=True)
        with ThreadPoolExecutor(max_workers=8) as pool:
            pages = list(pool.map(lambda _: https(port, "example.com"), range(8)))
        assert all(b"Example Domain" in page for page in pages)
        print("PASS: eight concurrent HTTPS connections", flush=True)
        for host in ["127.0.0.1", "localhost", "192.168.1.1", "::1", "::ffff:127.0.0.1"]:
            with socks(port, host, 80, expected=2):
                pass
        print("PASS: phone loopback and private destinations rejected", flush=True)
        with socks(port, "example.com", 443, command=3, expected=7):
            pass
        print("PASS: unsupported UDP rejected", flush=True)
        if args.upload:
            payload = (b"transbox-prototype-upload-" * 2700)[:65536]
            echoed = json.loads(https(port, args.upload_host, "/post", payload))
            assert echoed["data"].encode() == payload
            print("PASS: 64 KiB HTTPS upload and response", flush=True)
        # Remove only the new forwarding rule owned by this launcher, simulating a broken cable.
        serial, local, _ = owned_forward
        assert serial == device
        subprocess.run([args.adb, "-s", device, "forward", "--remove", local], check=True)
        assert process.wait(timeout=20) != 0
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
        except OSError:
            pass
        else:
            raise AssertionError("Proxy still accepting connections after tunnel loss")
        assert forward_list(args.adb, device) == before
        print("PASS: tunnel loss stops launcher/proxy and cleans its forwarding rule", flush=True)
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
            process.wait(timeout=20)
        # Even a failed assertion should leave unrelated ADB state untouched.
        if owned_forward is not None and owned_forward in forward_list(args.adb, device):
            serial, local, _ = owned_forward
            subprocess.run([args.adb, "-s", device, "forward", "--remove", local], check=False)


if __name__ == "__main__":
    main()
