# Third-party components

The MIT license in this repository applies to Transbox's own source. Bundled
upstream components retain their own licenses, copyrights, and notices.

| Component | Source / notice |
| --- | --- |
| Chromium | [Source](https://chromium.googlesource.com/chromium/src/), [bundled license](scripts/CHROMIUM-LICENSE); dependency notices at `chrome://credits` |
| Android Platform Tools / ADB | [Source](https://android.googlesource.com/platform/packages/modules/adb/); upstream `NOTICE.txt` is included in the runtime |
| Rust dependencies | `Cargo.lock`; available package license/notice files and metadata are collected during packaging |
| Rust toolchain and LLVM-MinGW runtime | Available compiler/runtime notices are collected into `licenses/`; pinned versions are recorded in the toolchain file and download lock |

Packaging produces a separate notices archive and embeds the available notices
in the executable's runtime payload. `sources.json` identifies upstream archives
and verified hashes. Chromium dependency notices remain accessible offline at
`chrome://credits`.

Retain notices and inspect licensing changes when updating dependencies. The
collector preserves files available in resolved Cargo packages; it does not
replace reviewing a changed dependency's requirements.
