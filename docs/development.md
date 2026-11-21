# Development

## Toolchain

- Linux x86_64 for packaged Windows builds.
- Rust/rustup; `rust-toolchain.toml` selects the tested compiler and components.
- Python 3.12+; build and release scripts use only its standard library.
- Android NDK **29.0.13846066**, the version used by release automation.
- ADB and Chromium on the Linux host for live development testing.

The build script finds an NDK under `~/Android/Sdk/ndk`, `ANDROID_HOME`, or
`ANDROID_SDK_ROOT`. Override with `--ndk PATH`, `ANDROID_NDK_HOME`, or
`ANDROID_NDK_ROOT`. Release automation passes the exact NDK path explicitly.

```sh
python3 scripts/build.py --ndk "$HOME/Android/Sdk/ndk/29.0.13846066"
```

The script builds four Android relays at API 24, prepares the Chromium/ADB
payload, and cross-compiles the launcher for `x86_64-pc-windows-gnullvm`.
LLVM-MinGW is downloaded to `.tools` if a suitable linker is not available.
The Windows target config links the launcher runtime statically, including the
unwinder, so it can start without a sidecar DLL before extracting its payload.
Use `--abis arm64-v8a` for a smaller development build; release automation uses
all four architectures.

Archive URLs and SHA256 hashes are pinned in `scripts/downloads.lock.json`;
Cargo dependencies are locked in `Cargo.lock`. Downloads with mismatching hashes
are rejected. Build metadata records the compiler, NDK, source commit when
available, and bundled dependency versions. Pinned inputs improve repeatability;
byte-for-byte reproducibility is not claimed.

## Local development

```sh
python3 scripts/build.py --dev --abis arm64-v8a
target/debug/transbox --runtime .payload/dev --adb /usr/bin/adb --browser /usr/bin/chromium
```

An ordinary `cargo build` creates a launcher without embedded assets. Use
`--runtime PATH` for an unpacked payload, or set `TRANSBOX_BUNDLE` to an absolute
ZIP path at compile time. `--adb PATH` and `--browser PATH` override development
executables. CLI reference: `cargo run -p transbox -- --help`.

## Automated checks

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 -m unittest discover -s tests -p 'test_*.py'
python3 scripts/release.py check
```

GitHub CI runs these checks on Linux and Windows. No phone or external test
website is required. Release automation additionally builds all Android ABIs and
exercises the packaged executable's version and ADB listing on Windows. That
smoke check does not validate physical USB hardware or browser rendering.

## Live phone checks

```sh
python3 tests/phone_smoke.py --upload
target/debug/transbox --runtime .payload/dev --adb /usr/bin/adb \
  --browser /usr/bin/chromium --headless --duration 30
```

The opt-in smoke test uses the connected phone's Internet for HTTPS, eight
concurrent connections, private-destination denial, UDP rejection, an optional
generated 64 KiB upload to `postman-echo.com`, and tunnel-loss shutdown. It removes
only its own new ADB forward to simulate failure. It does not disable networking.

Initial checks passed on an ARM64 Android API 36 phone with native Linux Chromium.
The Windows launcher also passed HTTPS, concurrency, upload, and tunnel-loss checks
under Wine using the Linux ADB server. Other Android architectures have been
cross-compiled but not tested on physical devices.

Before relying on a release, test the actual Windows PC with Wi-Fi/Ethernet
disconnected, then enable them and unplug USB during browsing. Capture IPv4 and
IPv6 traffic to investigate leaks. A proxy test does not prove OS-level isolation.
