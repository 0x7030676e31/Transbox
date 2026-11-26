# Development

Packaged builds use Linux x86_64, the pinned Rust toolchain, Python 3.12+, and
Android NDK 29.0.13846066. The build script finds an NDK under `~/Android/Sdk/ndk`
or the Android SDK environment variables; `--ndk PATH` overrides discovery.

```sh
python3 scripts/build.py --ndk "$HOME/Android/Sdk/ndk/29.0.13846066"
```

The script cross-compiles Android ARM64, ARMv7, x86_64 and x86 relays, then the
Windows x64 launcher. LLVM-MinGW is downloaded if needed. The launcher runtime
is statically linked so it starts before extracting its embedded payload.
Upstream archives are hash-verified; Cargo versions are recorded in `Cargo.lock`.
Build metadata includes the compiler, NDK, source commit and input hashes.
Byte-for-byte reproducibility is not claimed.

## Local build and checks

```sh
python3 scripts/build.py --dev --abis arm64-v8a
target/debug/transbox --runtime .payload/dev --adb /usr/bin/adb --browser /usr/bin/chromium

cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 -m unittest discover -s tests -p 'test_*.py'
python3 scripts/release.py check
```

An ordinary `cargo build` has no embedded runtime. `--runtime`, `--adb` and
`--browser` are development overrides. `TRANSBOX_BUNDLE` embeds a prebuilt ZIP
at compile time. `--data-dir` selects an isolated encrypted store. `--ephemeral`,
`--headless` and `--proxy-only` do not read or write that store; they are test modes.

CI runs tests and Clippy on Linux and Windows, including profile encryption,
recovery, locking and process lifetime. Release automation builds all Android
architectures and verifies packaged version, checksum and ADB startup on Windows.
Hosted runners have no phone; their checks do not validate physical USB or browser UI.

## Connected-phone tests

Run these sequentially with an authorized USB phone:

```sh
python3 tests/phone_smoke.py --upload
xvfb-run -a -s '-screen 0 1000x800x24 -nolisten tcp' \
  python3 tests/gui_smoke.py --screenshots .tools/validation/gui
```

The first checks phone-side DNS, TCP HTTP/HTTPS, concurrent requests, upload,
private-destination rejection, UDP rejection and tunnel-loss cleanup.
The GUI test additionally checks password setup/unlock/reset, persisted history,
multiple browser windows, duplicate launch handling, WSS echo, WebRTC UDP
restrictions, and browser transport logs. It needs Xvfb, xdotool, ImageMagick,
installed Chromium and access to the public test endpoints. All browser data and
test passwords are synthetic, in a temporary store; ordinary browser data is untouched.
Chromium debugging and network logging are enabled only by the test wrapper.

UDP socket creation is not itself evidence of a transmitted packet: Chromium
also uses UDP sockets for local route/address probes. The test checks datagram
send/receive events and ICE candidates, rather than treating bind/connect events
as traffic. This observation does not replace OS-enforced isolation.

Before relying on a release, also test the actual Windows PC with its Internet
disconnected, and unplug USB during browsing. Only ARM64 has been exercised on
a physical phone; other bundled Android architectures are cross-compiled.
