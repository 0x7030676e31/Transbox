# Changelog

Changes are recorded by version. Transbox uses Semantic Versioning; `0.x` releases
are experimental and may change behavior or interfaces between minor versions.

## [Unreleased]

## [0.1.0-alpha.2] - 2026-11-22

First published alpha: a bundled Windows browser using an Android phone's Internet over USB.

### Fixed

- Start the bundled ADB server automatically on a fresh PC, while ignoring remote-server environment overrides.
- Set up Android SDK tools explicitly on the release runner.

### Limitations

- Proxy routing only; no OS-enforced leak protection or automatic browser updates.
- Physical Windows USB behavior and browser rendering still need real-machine validation.
- Follows the phone's selected network; does not force cellular or select a SIM.

## [0.1.0-alpha.1] - 2026-11-22

Unpublished: the Windows first-launch smoke test failed. The tag is retained unchanged.

### Added

- Rust Windows launcher, local SOCKS5 broker, and authenticated Android TCP relay.
- Single executable bundling Chromium, ADB, its Windows DLLs, and four Android CPU architectures.
- Separate temporary browser profile and phone-side destination DNS resolution.
- USB transport selection, heartbeat monitoring, and browser shutdown on tunnel loss.
- Private destination rejection and TCP half-close handling.
- Linux development build and opt-in tests against a connected Android phone.
- Pinned build dependencies, CI checks, versioned assets, checksums, and draft release automation.

### Known limitations

- Proxy configuration only; Windows WFP enforcement and a hard no-leak guarantee are absent.
- Native Windows USB behavior and browser rendering still need real-machine validation.
- Uses the phone's selected network; cannot force cellular or select a SIM.
- The bundled Chromium snapshot does not update automatically.
- Linux graphics/font warnings and repeated TLS protocol failures have been reported;
  the affected websites and causes have not yet been established.
