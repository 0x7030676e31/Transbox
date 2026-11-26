# Transbox

A Rust launcher that runs Chromium through an Android phone's Internet connection
over USB ADB. The Windows executable bundles Chromium, ADB, and native Android
relays. Other applications keep their normal connection.

Experimental (`0.2.0-alpha.2`). Browser routing uses proxy settings, without
OS-enforced leak protection.

## Use

Enable USB debugging, connect a data-capable USB cable, and accept the phone's
authorization prompt. A working ADB USB driver is required on Windows, as with
scrcpy. No root or APK installation is needed.

Launch the executable. On first use, create a local storage password; later,
enter it to unlock your saved history, logins and settings. The launcher window
hides while Chromium is open. All windows share one profile and phone tunnel.
Closing the last window saves and encrypts the profile before Transbox exits.

```powershell
.\transbox-v0.2.0-alpha.2-windows-x64.exe
.\transbox-v0.2.0-alpha.2-windows-x64.exe --device SERIAL --url https://example.org
```

Storage is under `%LOCALAPPDATA%\Transbox\storage`; `--data-dir PATH` overrides it.
**Reset local storage** deletes the profile and backup after confirmation. There
is no password recovery. See [storage and recovery](docs/storage.md).

The phone uses its selected network; disable phone Wi-Fi to use cellular data.
First launch extracts the bundled runtime into `%LOCALAPPDATA%\Transbox\runtime`.
`TRANSBOX_CACHE_DIR` changes that cache location. Builds are unsigned; release
assets include a checksum, build metadata, and dependency notices.

## Build

Requires Linux x86_64, Rust/rustup, Python 3.12+, and Android NDK 29.0.13846066.
The Rust toolchain is pinned. The script downloads verified build dependencies.

```sh
python3 scripts/build.py
```

Versioned assets go into `dist/`. For Linux development with installed ADB and Chromium:

```sh
python3 scripts/build.py --dev --abis arm64-v8a
target/debug/transbox --runtime .payload/dev --adb /usr/bin/adb --browser /usr/bin/chromium
```

## Limits

- Windows 10/11 x64 is the release target; Android 7+ is required. ARM64 is tested
  on a real phone; the other bundled Android architectures are cross-compiled.
- Browser TCP traffic uses the phone, including HTTP(S) and WebSockets. Chromium's
  SOCKS path does not support UDP: HTTP/3 is disabled and direct WebRTC UDP is
  blocked. There is no direct fallback. Some real-time sites may not work.
- Private network destinations are blocked. The browser does not update itself.
- Working profile files are decrypted while the browser is open. An interrupted
  shutdown can leave those files for recovery. This is not protection against
  an administrator or malware on the computer, or a guarantee of secure erasure.

[Development](docs/development.md) · [Architecture](docs/architecture.md) ·
[Troubleshooting](docs/troubleshooting.md) · [Releases](docs/releasing.md) · [Changelog](CHANGELOG.md)

[MIT](LICENSE). Bundled dependencies retain their [own licenses](THIRD_PARTY.md).
