# Architecture

```text
Password window -> encrypted local profile -> one browser session
Chromium windows -> local SOCKS5 broker -> authenticated ADB USB tunnel
                  -> Android Rust relay -> phone DNS and outbound TCP -> website
```

| Crate | Responsibility |
| --- | --- |
| `transbox` | Unlock GUI, device selection, session controller, broker and browser lifecycle |
| `transbox-storage` | Profile encryption, OS-held lock, atomic saves, backups and recovery |
| `transbox-core` | Authentication framing, SOCKS5, destination filtering and TCP forwarding |
| `transbox-relay` | Native Android executable run as the ADB shell user |

## Session and routing

The controller locks storage before decryption. A second launcher can only
request focus through a local authenticated control channel; it cannot start a
second writer. No password or profile data is sent through that channel.

One Chromium browser session owns all tabs, windows, renderer and network
processes. New windows use the existing profile and proxy configuration. On
Windows, Chromium starts suspended and is assigned to a kill-on-close job before
it can create children. The controller retains the tunnel and storage lock until
that job is empty. Linux development uses a process group and checks for remaining
processes before saving or recovering working files.

The phone relay is pushed to a unique `/data/local/tmp/transbox-<session>` path,
binds phone loopback, and is reached with one dynamic `adb forward`. Each stream
uses a random 256-bit session token. The launcher pins a verified USB transport
ID and cannot switch to wireless ADB with the same serial number.

The relay resolves destinations on Android and rejects private, loopback,
link-local, multicast and selected reserved addresses. It forwards TLS bytes
unchanged: Chromium validates website certificates, without a custom CA.

Chromium uses one fixed SOCKS5 proxy with no direct fallback. Desktop destination
DNS lookups and implicit loopback proxy bypass are disabled. All browser windows
share this configuration. Chromium SOCKS supports TCP URL traffic, not UDP;
HTTP/3 is disabled and non-proxied WebRTC UDP is blocked. Arbitrary browser UDP
would require a different transport/routing design.

The desktop broker is loopback-only without client authentication, so other local
applications can use it while running. Browser proxy settings are not a firewall
or a boundary against malicious software on the desktop. Chromium's own sandbox
stays enabled. ADB authorization grants broader debugging access to the phone.

## Shutdown

The controller checks the tunnel every two seconds. Socket timeouts can add to
failure detection latency. On failure it requests browser shutdown, then forces
remaining tracked processes to exit if necessary. Profile files are saved only
after browser processes have stopped. Failed saves retain working files and
report that cleanup is incomplete. See [storage and recovery](storage.md).

The relay expires after roughly 16 seconds without heartbeats. Cleanup removes
only this session's forwarding rule and relay file; the shared ADB server is left
running for other tools. Disconnecting USB may prevent removing the phone file.
The extracted runtime cache stays on disk for reuse.
