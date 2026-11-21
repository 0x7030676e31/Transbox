# Architecture

```text
Dedicated Chromium with an ephemeral profile
  -> loopback SOCKS5 broker in Rust launcher
  -> authenticated session through adb forward over USB
  -> Rust relay on Android: destination DNS and outbound TCP
  -> phone's selected network -> website
```

The browser is the upstream Chromium interface launched by Rust. There is no
custom CEF interface. The launcher, broker, protocol, and Android relay are Rust.

| Crate | Responsibility |
| --- | --- |
| `transbox` | Runtime extraction, device selection, broker, browser lifecycle |
| `transbox-core` | Authentication, SOCKS5, public destination checks, bidirectional TCP |
| `transbox-relay` | Native relay executed as the Android ADB shell user |

## Connection lifecycle

The launcher uses the local ADB server on port 5037. It accepts only USB devices
and pins the chosen transport ID, preventing a switch to a wireless ADB transport
with a matching serial number.

It pushes an architecture-specific executable to a unique
`/data/local/tmp/transbox-<session>` path and runs it through ADB. The relay binds
phone loopback on a dynamically allocated port. One ADB forward connects a
desktop loopback port to that listener. Each session generates a fresh 256-bit
token; forwarded streams authenticate with it before the SOCKS handshake.

The desktop SOCKS listener is loopback-only and does not authenticate local
clients. Other local applications can use it during the session. This is not a
boundary against malicious software already running on the computer. ADB
authorization itself grants broad debugging access to the phone.

Destination hostnames are resolved on Android. The relay filters resolved
addresses and rejects private, loopback, link-local, multicast, and selected
reserved destinations. HTTPS bytes pass through unchanged: the browser validates
the site's certificate and no CA is installed.

## Browser configuration

Each session has a separate temporary profile. Permission defaults deny camera,
microphone, location, notifications, and selected device APIs. Downloads prompt
for a destination. The launcher does not import normal browser data.

There is one fixed SOCKS5 proxy, without a direct fallback. QUIC is disabled,
non-proxied WebRTC UDP is disabled, implicit loopback proxy bypass is removed,
and desktop hostname resolution is configured to fail except for the numeric
loopback proxy endpoint. These settings do not constitute OS-level enforcement.

Chromium's sandbox remains enabled. **No Windows WFP/firewall enforcement is
implemented.** A hard guarantee that all browser traffic uses the phone requires
that additional boundary and validation on IPv4, IPv6, TCP, UDP, and DNS paths.

## Failure and cleanup

The launcher checks the tunnel every two seconds and stops the browser on failure.
Blocking socket operations can add their timeout to detection latency. There is
no automatic reconnect or attempt to use Windows Internet directly.

The relay exits after 15 seconds without authenticated heartbeats, or about
16 seconds allowing for its one-second clock. The same lease applies at startup.
Windows uses a kill-on-close job for browser cleanup; Linux uses a process group.

Cleanup removes only this session's forward and relay file. The shared ADB server
is left running for tools such as scrcpy. USB disconnection can prevent removal
of the phone file, but the relay process still expires. Hard termination can
prevent some cleanup, including removal of forwarding rules.

The content-addressed runtime cache remains on disk so ADB's shared server can
continue using its executable and subsequent launches avoid extraction. Delete
unused cache versions only when their ADB server and browser are no longer running.

## Future work

- OS-enforced Windows network isolation, with explicit IPv4/IPv6 leak tests.
- Real Windows USB, browser lifecycle, rendering, and driver validation.
- A maintained Chromium release/update policy.
- Connection/setup UI and reconnect behavior without any direct fallback.
- An optional Android APK for explicit cellular/SIM selection and managed background operation.
