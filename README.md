# Transbox

A Rust launcher that runs Chromium through an Android phone's Internet connection
over USB ADB. The Windows executable bundles the browser, ADB, and native Android
relays. Other applications keep using their normal connection.

Experimental (`0.1.0-alpha.1`). Routing uses browser proxy settings, without
OS-enforced leak protection. Native Windows USB behavior and browser rendering
still need real-machine validation.

## Use

Download the Windows x64 executable from **Releases**, connect your phone with a
data-capable USB cable, enable USB debugging, accept the authorization prompt,
and launch it. A working ADB USB driver is required, just as with scrcpy.
No root or APK installation is needed.

```powershell
.\transbox-v0.1.0-alpha.1-windows-x64.exe
.\transbox-v0.1.0-alpha.1-windows-x64.exe --device SERIAL --url https://example.org
```

Close the browser or press Ctrl+C to stop. Each session uses a temporary profile.
The phone uses its selected network; disable phone Wi-Fi to use cellular data.

The download is about 211 MiB and extracts roughly 500 MiB on first launch into
`%LOCALAPPDATA%\Transbox\runtime`. Set `TRANSBOX_CACHE_DIR` to change that location.
Builds are unsigned; a matching `.exe.sha256` file accompanies each release.

## Build

Requires Linux x86_64, Rust/rustup, Python 3.12+, and Android NDK 29.0.13846066.
The Rust toolchain is pinned; the script downloads verified build dependencies.

```sh
python3 scripts/build.py
```

Versioned assets go into `dist/`. For Linux development with installed ADB and Chromium:

```sh
python3 scripts/build.py --dev --abis arm64-v8a
target/debug/transbox --runtime .payload/dev --adb /usr/bin/adb --browser /usr/bin/chromium
```

## Limitations

- Windows 10/11 x64 is the intended target; Android 7+ is required. ARM64 has been
  tested on a real phone; other bundled Android architectures have only been compiled.
- TCP traffic only. QUIC is disabled, direct WebRTC UDP is restricted, and private
  network destinations are blocked.
- Chromium's sandbox stays enabled, but proxy settings do not guarantee that
  every connection uses the phone. The bundled browser does not update itself.
- TLS and Linux graphics/font errors have been reported; see [troubleshooting](docs/troubleshooting.md).

[Development](docs/development.md) · [Architecture](docs/architecture.md) ·
[Releases](docs/releasing.md) · [Changelog](CHANGELOG.md)

[MIT](LICENSE). Bundled dependencies retain their [own licenses](THIRD_PARTY.md).
