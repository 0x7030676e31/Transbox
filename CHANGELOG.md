# Changelog

Changes are recorded by version. Transbox uses Semantic Versioning; `0.x` releases
are experimental and may change behavior or interfaces between minor versions.

## [Unreleased]

## [0.2.0-alpha.1] - 2026-11-26

### Added

- Password setup/unlock GUI and encrypted local browser history, logins and settings.
- Argon2id key derivation, authenticated profile encryption, atomic saves and an encrypted backup.
- Explicit recovery of interrupted sessions and confirmed local-storage reset.
- One active writer per store; additional browser windows share the profile and phone tunnel.
- Suspended Windows browser startup with process-job tracking before children can launch.

### Changed

- Upgrade Rust and older Rust dependencies; preserve existing Chromium preferences.
- Hide the launcher while browsing and save only after tracked browser processes finish.
- Remove temporary network/graphics reports from troubleshooting; document only actionable issues.

### Protocols

- TCP HTTP(S), uploads and WebSockets use the phone. Unsupported browser UDP is blocked;
  Chromium SOCKS cannot tunnel UDP, so HTTP/3 remains disabled.

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
