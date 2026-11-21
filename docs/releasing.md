# Releases

The version lives in `[workspace.package].version` in `Cargo.toml`; all crates
inherit it. Tags use `vVERSION`, starting with `v0.1.0-alpha.1`.

## Next version

1. Update the workspace version and run `cargo update --workspace --offline`.
2. Add a dated entry to `CHANGELOG.md`, moving changes from `[Unreleased]`.
3. Update README version/examples and run the [checks](development.md#automated-checks).
4. Commit and push the source, then tag it:

```sh
git tag -a v0.1.0-alpha.1 -m "Transbox 0.1.0-alpha.1"
git push origin v0.1.0-alpha.1
```

Use a new version/tag for each release. `0.x` versions are experimental;
prereleases use suffixes such as `-alpha.2`.

## Automation

- **CI** runs checks on Linux and Windows.
- **Build and release → Run workflow** builds downloadable Actions artifacts.
- Pushing a `v*` tag validates the version/changelog, builds the bundle, checks
  startup on Windows, and creates a **draft release**. Prerelease tags are marked
  accordingly. Inspect and publish the draft on GitHub.

The Windows runner has no phone, so its smoke test covers startup and bundled ADB,
not physical USB or browser rendering. The workflows need a first push to run.

## Local build

```sh
python3 scripts/release.py check --tag v0.1.0-alpha.1
python3 scripts/build.py
```

`dist/` contains the versioned executable, `.exe.sha256`, `.build.json` with build
inputs and hashes, and `.notices.zip` with dependency notices. Verify from `dist/`:

```sh
sha256sum -c transbox-v0.1.0-alpha.1-windows-x64.exe.sha256
```

Builds are unsigned. Review bundled Chromium/ADB versions before publishing;
the browser does not update automatically.
