#!/usr/bin/env python3
"""Opt-in Linux/X11 GUI + real-phone test. Run under xvfb-run.

Uses only a temporary profile and a generated test password. Requires xdotool,
Chromium, and an authorized USB phone. No changes to your ordinary browser data.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import sqlite3
import struct
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
PASSWORD = "transbox-test-" + secrets.token_hex(12)


def wait_for(fn, message, timeout=40):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = fn()
        if result:
            return result
        time.sleep(0.15)
    raise AssertionError(message)


def xdo(*args, check=True):
    return subprocess.run(["xdotool", *map(str, args)], check=check, text=True, capture_output=True).stdout.strip()


def windows(pattern):
    return xdo("search", "--onlyvisible", "--name", pattern, check=False).splitlines()


def click(window, x, y):
    xdo("windowfocus", "--sync", window)
    xdo("mousemove", "--window", window, x, y, "click", 1)


def type_text(text):
    xdo("type", "--clearmodifiers", "--delay", 3, text)


def exact(stream, size):
    data = bytearray()
    while len(data) < size:
        chunk = stream.recv(size - len(data))
        if not chunk:
            raise RuntimeError("CDP connection closed")
        data.extend(chunk)
    return bytes(data)


class CDP:
    def __init__(self, port, path):
        self.socket = socket.create_connection(("127.0.0.1", port), timeout=25)
        self.counter = 0
        key = base64.b64encode(secrets.token_bytes(16)).decode()
        self.socket.sendall((f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        header = bytearray()
        while not header.endswith(b"\r\n\r\n"):
            header.extend(exact(self.socket, 1))
        assert b"101 WebSocket" in header or b"101 Switching" in header, header

    def send(self, payload, opcode=1):
        mask = secrets.token_bytes(4)
        size = len(payload)
        prefix = bytes([0x80 | opcode])
        if size < 126:
            prefix += bytes([0x80 | size])
        elif size < 65536:
            prefix += b"\xfe" + struct.pack("!H", size)
        else:
            prefix += b"\xff" + struct.pack("!Q", size)
        self.socket.sendall(prefix + mask + bytes(value ^ mask[i % 4] for i, value in enumerate(payload)))

    def receive(self):
        combined = bytearray()
        while True:
            first, second = exact(self.socket, 2)
            size = second & 127
            if size == 126:
                size = struct.unpack("!H", exact(self.socket, 2))[0]
            elif size == 127:
                size = struct.unpack("!Q", exact(self.socket, 8))[0]
            assert size < 10 * 1024 * 1024
            mask = exact(self.socket, 4) if second & 128 else None
            data = exact(self.socket, size)
            if mask:
                data = bytes(value ^ mask[i % 4] for i, value in enumerate(data))
            if first & 15 == 9:
                self.send(data, opcode=10)
                continue
            if first & 15 == 8:
                raise RuntimeError("CDP closed")
            combined.extend(data)
            if first & 128:
                return json.loads(combined)

    def call(self, method, params=None, session=None):
        self.counter += 1
        message = {"id": self.counter, "method": method, "params": params or {}}
        if session:
            message["sessionId"] = session
        self.send(json.dumps(message).encode())
        while True:
            reply = self.receive()
            if reply.get("id") == self.counter:
                assert "error" not in reply, reply
                return reply.get("result", {})

    def evaluate(self, session, expression):
        result = self.call("Runtime.evaluate", {"expression": expression, "returnByValue": True, "awaitPromise": True}, session)
        assert "exceptionDetails" not in result, result
        return result["result"].get("value")

    def close(self):
        self.socket.close()


def connect_browser(storage):
    path = storage / "working/DevToolsActivePort"
    lines = wait_for(lambda: path.read_text().splitlines() if path.exists() else None, "Browser debugging pipe did not appear")
    return CDP(int(lines[0]), lines[1])


def browser_pages(cdp):
    return [target for target in cdp.call("Target.getTargets")["targetInfos"] if target["type"] == "page"]


def attach(cdp, target):
    return cdp.call("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]


def title_loaded(cdp, session):
    return cdp.evaluate(session, "document.title") == "Example Domain"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/transbox")
    parser.add_argument("--runtime", type=Path, default=ROOT / ".payload/dev")
    parser.add_argument("--adb", default=shutil.which("adb"))
    parser.add_argument("--browser", default=shutil.which("chromium"))
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    assert os.getenv("DISPLAY"), "Run with xvfb-run -a python3 tests/gui_smoke.py"
    assert args.adb and args.browser, "ADB and Chromium are required"
    with tempfile.TemporaryDirectory(prefix="transbox-gui-test-") as folder:
        root = Path(folder)
        storage = root / "storage"
        netlog = root / "netlog.json"
        wrapper = root / "chromium-test-wrapper"
        wrapper.write_text("#!/usr/bin/env python3\nimport os,sys\nos.execv(" + repr(args.browser) + ", [" + repr(args.browser) + ", '--remote-debugging-port=0', '--enable-automation', " + repr("--log-net-log=" + str(netlog)) + ", *sys.argv[1:]])\n")
        wrapper.chmod(0o700)
        env = os.environ.copy()
        env.pop("WAYLAND_DISPLAY", None)
        env.update({"XDG_CONFIG_HOME": str(root / "config"), "XDG_RUNTIME_DIR": str(root / "run"), "XDG_SESSION_TYPE": "x11", "LIBGL_ALWAYS_SOFTWARE": "1"})
        (root / "run").mkdir(mode=0o700)
        cmd = [str(args.binary.resolve()), "--data-dir", str(storage), "--runtime", str(args.runtime.resolve()), "--adb", args.adb, "--browser", str(wrapper), "--url", "https://example.com/?transbox-window=first"]
        log = (root / "launcher.log").open("w+")
        process = None
        cdp = None

        def start():
            return subprocess.Popen(cmd, env=env, stdout=log, stderr=log, cwd=ROOT)

        def screenshot(name):
            if args.screenshots:
                args.screenshots.mkdir(parents=True, exist_ok=True)
                subprocess.run(["import", "-window", "root", str(args.screenshots / (name + ".png"))], check=True)

        def unlock(password):
            window = wait_for(lambda: windows("^Transbox$"), "Unlock window did not appear")[0]
            click(window, 120, 220)
            xdo("key", "ctrl+a")
            type_text(password)
            xdo("key", "Return")

        try:
            process = start()
            window = wait_for(lambda: windows("^Transbox$"), "Setup window did not appear")[0]
            screenshot("setup")
            click(window, 120, 220)
            type_text(PASSWORD)
            xdo("key", "Tab")
            type_text(PASSWORD)
            xdo("key", "Return")
            cdp = connect_browser(storage)
            target = wait_for(lambda: next((t for t in browser_pages(cdp) if "transbox-window=first" in t["url"]), None), "Initial page missing")
            session = attach(cdp, target["targetId"])
            wait_for(lambda: title_loaded(cdp, session), "Initial HTTPS page did not load")
            assert not windows("^Transbox$"), "Unlock window is still visible"
            print("PASS: GUI setup opens browser through phone and hides launcher", flush=True)
            arguments = cdp.call("Browser.getBrowserCommandLine")["arguments"]
            assert "--disable-quic" in arguments
            assert "--force-webrtc-ip-handling-policy=disable_non_proxied_udp" in arguments
            assert any(a.startswith("--proxy-server=socks5://127.0.0.1:") for a in arguments)
            second = cdp.call("Target.createTarget", {"url": "https://example.com/?transbox-window=second", "newWindow": True})["targetId"]
            second_session = attach(cdp, second)
            wait_for(lambda: title_loaded(cdp, second_session), "Second window did not load through proxy")
            duplicate = subprocess.run(cmd, env=env, stdout=log, stderr=log, timeout=12)
            assert duplicate.returncode == 0
            assert len(browser_pages(cdp)) == 2 and not windows("^Transbox$"), "Second launcher created another session"
            cdp.call("Target.closeTarget", {"targetId": target["targetId"]})
            time.sleep(0.4)
            assert process.poll() is None and (storage / "working").exists()
            print("PASS: additional window shares session; duplicate launcher does not decrypt a second copy", flush=True)
            ice = cdp.evaluate(second_session, """(async () => {
                const pc = new RTCPeerConnection({iceServers:[{urls:'stun:stun.l.google.com:19302'}]});
                const candidates=[]; pc.onicecandidate=e=>{if(e.candidate)candidates.push(e.candidate.candidate)};
                pc.createDataChannel('probe'); await pc.setLocalDescription(await pc.createOffer());
                await new Promise(r=>setTimeout(r,4000)); pc.close(); return candidates;
            })()""")
            assert all(" udp " not in candidate.lower() for candidate in ice), ice
            print("PASS: browser WebRTC probe offers no direct UDP candidates; HTTP/3 disabled", flush=True)
            echo = cdp.evaluate(second_session, """new Promise(resolve=>{
                const w=new WebSocket('wss://echo.websocket.org');
                const timer=setTimeout(()=>{w.close();resolve(false)},12000);
                w.onopen=()=>w.send('transbox-websocket-probe');
                w.onmessage=e=>{if(e.data==='transbox-websocket-probe'){clearTimeout(timer);w.close();resolve(true)}};
                w.onerror=()=>{clearTimeout(timer);resolve(false)};
            })""")
            assert echo, "Public WebSocket echo failed"
            print("PASS: browser WSS echo through the phone", flush=True)
            cdp.call("Target.closeTarget", {"targetId": second})
            cdp.close(); cdp = None
            assert process.wait(timeout=30) == 0
            assert (storage / "profile.vault").is_file() and not (storage / "working").exists()
            sealed = (storage / "profile.vault").read_bytes()
            assert PASSWORD.encode() not in sealed and b"transbox-window=first" not in sealed
            # Transport logging records actual TCP socket connects, not destination URLs.
            network = json.loads(netlog.read_text())
            kinds = network["constants"]["logEventTypes"]
            tcp_type = kinds.get("TCP_CONNECT_ATTEMPT")
            addresses = [e["params"]["address"] for e in network["events"] if e["type"] == tcp_type and e.get("params", {}).get("address")]
            assert addresses and all(a.startswith("127.0.0.1:") or a.startswith("[::1]:") for a in addresses), addresses
            # UDP connect()/bind() can be route/local-address probes without a
            # transmitted packet. Check datagrams, not socket creation alone.
            udp_kinds = {v for k, v in kinds.items() if k in {"UDP_BYTES_SENT", "UDP_BYTES_RECEIVED", "UDP_SEND_ERROR"}}
            assert not any(e["type"] in udp_kinds for e in network["events"]), "Browser attempted UDP datagram traffic"
            print("PASS: browser TCP connections use loopback proxy; no UDP datagrams in transport log", flush=True)
            print("PASS: closing last window encrypts storage and removes working files", flush=True)
            process = start()
            unlock("incorrect-test-password")
            time.sleep(4)
            assert process.poll() is None and not (storage / "working").exists()
            assert (storage / "profile.vault").read_bytes() == sealed
            screenshot("locked")
            unlock(PASSWORD)
            cdp = connect_browser(storage)
            wait_for(lambda: browser_pages(cdp), "Saved profile did not reopen")
            history = storage / "working/Default/History"
            def persisted():
                if not history.exists():
                    return False
                try:
                    with sqlite3.connect(history.as_uri() + "?mode=ro", uri=True) as db:
                        rows = db.execute("select url from urls where url like '%transbox-window=%'").fetchall()
                    return any("second" in row[0] for row in rows)
                except sqlite3.OperationalError:
                    return False
            wait_for(persisted, "History from second window was not persisted")
            print("PASS: wrong password leaves vault intact; correct password restores browser history", flush=True)
            for page in browser_pages(cdp):
                cdp.call("Target.closeTarget", {"targetId": page["targetId"]})
            cdp.close(); cdp = None
            assert process.wait(timeout=30) == 0
            process = start()
            window = wait_for(lambda: windows("^Transbox$"), "Reset window missing")[0]
            # Locked form: password field, backup checkbox, primary button, reset link.
            click(window, 104, 350)
            time.sleep(0.3)
            screenshot("reset")
            click(window, 100, 245)
            type_text("RESET")
            click(window, 125, 280)
            wait_for(lambda: not (storage / "profile.vault").exists(), "Reset did not delete vault")
            assert not (storage / "profile.backup").exists() and not (storage / "working").exists()
            print("PASS: explicit reset deletes vault, backup and working data", flush=True)
            process.send_signal(signal.SIGINT)
            assert process.wait(timeout=10) == 0
        except Exception:
            screenshot("failure")
            log.flush()
            print((root / "launcher.log").read_text()[-7000:])
            raise
        finally:
            if cdp:
                cdp.close()
            if process and process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=25)
                except subprocess.TimeoutExpired:
                    process.kill(); process.wait()
            log.close()
            if args.screenshots:
                for name in ["netlog.json", "launcher.log"]:
                    if (root / name).exists():
                        shutil.copy2(root / name, args.screenshots / name)


if __name__ == "__main__":
    main()
