#!/usr/bin/env python3
"""Build native Android relays and a self-contained Windows x64 launcher on Linux.

Only Python's standard library is needed. External downloads are pinned and hashed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import urllib.request
import zipfile
from release import check as check_release

ROOT = Path(__file__).resolve().parents[1]
LOCK = json.loads((ROOT / "scripts/downloads.lock.json").read_text())
ABIS = {
    "arm64-v8a": ("aarch64-linux-android", "aarch64-linux-android24-clang"),
    "armeabi-v7a": ("armv7-linux-androideabi", "armv7a-linux-androideabi24-clang"),
    "x86_64": ("x86_64-linux-android", "x86_64-linux-android24-clang"),
    "x86": ("i686-linux-android", "i686-linux-android24-clang"),
}


def run(args, **kwargs):
    print("+", " ".join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), cwd=ROOT, check=True, **kwargs)


def sha256(path):
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def download(name):
    item = LOCK[name]
    path = ROOT / ".downloads" / item["file"]
    path.parent.mkdir(exist_ok=True)
    if not path.exists():
        print(f"Downloading {name} {item['version']}…", flush=True)
        temporary = path.with_suffix(".part")
        with urllib.request.urlopen(item["url"], timeout=60) as source, temporary.open("wb") as target:
            shutil.copyfileobj(source, target, length=1024 * 1024)
        temporary.replace(path)
    if sha256(path) != item["sha256"]:
        raise RuntimeError(f"SHA256 mismatch: {path}. Delete this file and retry; do not bypass verification.")
    return path


def find_ndk(explicit):
    for value in (explicit, os.getenv("ANDROID_NDK_HOME"), os.getenv("ANDROID_NDK_ROOT")):
        if value:
            return Path(value).expanduser().resolve()
    sdk = Path(os.getenv("ANDROID_HOME", os.getenv("ANDROID_SDK_ROOT", str(Path.home() / "Android/Sdk"))))
    candidates = sorted((sdk / "ndk").glob("*"), key=lambda p: tuple(int(n) for n in p.name.split(".") if n.isdigit()))
    if not candidates:
        raise RuntimeError("Android NDK not found. Install it through Android Studio, or pass --ndk PATH.")
    return candidates[-1]


def build_relays(ndk, abis, destination):
    toolchain = ndk / "toolchains/llvm/prebuilt/linux-x86_64/bin"
    for abi in abis:
        target, compiler = ABIS[abi]
        if not (toolchain / compiler).is_file():
            raise RuntimeError(f"NDK compiler missing: {toolchain / compiler}")
        run(["rustup", "target", "add", target])
        env = os.environ.copy()
        env[f"CARGO_TARGET_{target.upper().replace('-', '_')}_LINKER"] = str(toolchain / compiler)
        run(["cargo", "build", "--locked", "--release", "-p", "transbox-relay", "--target", target], env=env)
        folder = destination / "relays" / abi
        folder.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "target" / target / "release/transbox-relay", folder / "transbox-relay")


def windows_linker():
    override = os.getenv("CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER")
    if override:
        return override
    installed = shutil.which("x86_64-w64-mingw32-clang")
    if installed:
        return installed
    folder = ROOT / ".tools/llvm-mingw-20261117-ucrt-ubuntu-22.04-x86_64"
    linker = folder / "bin/x86_64-w64-mingw32-clang"
    if not linker.exists():
        archive = download("llvm-mingw-linux")
        folder.parent.mkdir(exist_ok=True)
        with tarfile.open(archive) as source:
            source.extractall(folder.parent, filter="data")
    return str(linker)


def prepare_windows(destination):
    destination.mkdir(parents=True, exist_ok=True)
    # Snapshot archives contain an enormous Chromium test executable. It is not a runtime dependency.
    with zipfile.ZipFile(download("chromium-win64")) as archive:
        for item in archive.infolist():
            relative = Path(item.filename).relative_to("chrome-win")
            if not relative.parts or item.is_dir() or relative.name in {"interactive_ui_tests.exe", "setup.exe"}:
                continue
            output = destination / "browser" / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(archive.read(item))
    with zipfile.ZipFile(download("adb-windows")) as archive:
        for name in ["adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll", "NOTICE.txt", "source.properties"]:
            output = destination / "adb" / name
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(archive.read("platform-tools/" + name))
    shutil.copy2(ROOT / "scripts/CHROMIUM-LICENSE", destination / "browser/LICENSE")
    shutil.copy2(ROOT / "LICENSE", destination / "TRANSBOX-LICENSE")
    (destination / "sources.json").write_text(json.dumps(LOCK, indent=2) + "\n")
    (destination / "THIRD-PARTY.txt").write_text(
        "Chromium: browser/LICENSE; dependency licenses are included in the browser at chrome://credits.\n"
        "Android Platform Tools: adb/NOTICE.txt. Chromium and ADB are unmodified upstream binaries.\n"
        "This is a prototype with a pinned Chromium snapshot, without automatic security updates.\n"
        "Source versions, download locations and SHA256 hashes: sources.json.\n"
        "Available Rust package and compiler/runtime notices: licenses/.\n"
    )


def collect_notices(destination):
    """Preserve available upstream Cargo notices alongside browser/ADB notices."""
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT, text=True
    ))
    packages = []
    for package in metadata["packages"]:
        if package["source"] is None:
            continue
        root = Path(package["manifest_path"]).parent
        files = []
        for path in root.rglob("*"):
            if not path.is_file() or path.is_symlink():
                continue
            relative = path.relative_to(root)
            if path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE")) or any(
                part.lower() in {"licenses", "licences"} for part in relative.parts[:-1]
            ):
                files.append(path)
        if package.get("license_file"):
            explicit = Path(package["license_file"])
            if not explicit.is_absolute():
                explicit = root / explicit
            if explicit.is_file() and explicit.is_relative_to(root):
                files.append(explicit)
        if not files:
            files.extend(root.glob("README*"))
        folder = destination / "licenses/rust" / f"{package['name']}-{package['version']}"
        for path in sorted(set(files)):
            if path.is_file():
                output = folder / path.relative_to(root)
                output.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, output)
        packages.append({key: package.get(key) for key in ("name", "version", "license", "repository", "source")})
    output = destination / "licenses/rust-packages.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(packages, indent=2) + "\n", encoding="utf-8")


def create_zip(folder, output, include=lambda _: True):
    # Stable ordering and timestamps avoid changing the payload cache key merely
    # because the same sources were unpacked at a different time.
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for path in sorted(folder.rglob("*")):
            if not path.is_file() or not include(path.relative_to(folder)):
                continue
            entry = zipfile.ZipInfo(path.relative_to(folder).as_posix(), (2026, 1, 1, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = (0o100755 if path.stat().st_mode & 0o111 else 0o100644) << 16
            with path.open("rb") as source, archive.open(entry, "w", force_zip64=True) as target:
                shutil.copyfileobj(source, target, length=1024 * 1024)


def collect_toolchain_notices(destination, linker):
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    docs = sysroot / "share/doc/rust"
    for path in [docs / "COPYRIGHT-library.html", *(docs / "licenses").rglob("*")]:
        if path.is_file():
            output = destination / "licenses/rust-toolchain" / path.relative_to(docs)
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, output)
    # The compiler itself is a build tool; its CRT/unwinder code is linked into
    # the executable, so retain the pinned toolchain's notices too.
    resolved = Path(shutil.which(linker) or linker).resolve()
    toolchain = resolved.parent.parent
    candidates = [toolchain / "LICENSE.TXT", *(toolchain / "x86_64-w64-mingw32/share/mingw32").glob("COPYING*")]
    for path in candidates:
        if path.is_file():
            output = destination / "licenses/llvm-mingw" / path.name
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, output)


def source_revision():
    result = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, capture_output=True)
    if result.returncode:
        return {"commit": None, "dirty": None}
    dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    return {"commit": result.stdout.strip(), "dirty": bool(dirty.strip())}


def release_assets(destination, release_version, ndk, abis, linker=None):
    stem = f"transbox-v{release_version}-windows-x64"
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    output = dist / (stem + ".exe")
    shutil.copy2(ROOT / "target/x86_64-pc-windows-gnullvm/release/transbox.exe", output)
    (dist / (output.name + ".sha256")).write_text(sha256(output) + "  " + output.name + "\n", encoding="utf-8")
    ndk_properties = (ndk / "source.properties").read_text(encoding="utf-8")
    ndk_version = next((line.split("=", 1)[1].strip() for line in ndk_properties.splitlines() if line.startswith("Pkg.Revision")), "unknown")
    info = {
        "version": release_version,
        "target": "x86_64-pc-windows-gnullvm",
        "android_abis": abis,
        "android_min_api": 24,
        "ndk_version": ndk_version,
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "windows_linker": subprocess.check_output([linker, "--version"], text=True).splitlines()[0] if linker else None,
        "source": source_revision(),
        "cargo_lock_sha256": sha256(ROOT / "Cargo.lock"),
        "payload_sha256": sha256(ROOT / ".payload/windows.zip"),
        "executable_sha256": sha256(output),
        "upstream": LOCK,
    }
    (dist / (stem + ".build.json")).write_text(json.dumps(info, indent=2) + "\n", encoding="utf-8")
    create_zip(destination, dist / (stem + ".notices.zip"), lambda path:
        path.parts[0] == "licenses" or path.as_posix() in {
            "browser/LICENSE", "adb/NOTICE.txt", "adb/source.properties", "sources.json", "THIRD-PARTY.txt", "TRANSBOX-LICENSE"
        })
    print(f"Ready: {output} ({output.stat().st_size / 1024**2:.1f} MiB)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ndk")
    parser.add_argument("--abis", default="arm64-v8a,armeabi-v7a,x86_64,x86")
    parser.add_argument("--dev", action="store_true", help="Build Linux launcher + Android relays, using system ADB/Chromium")
    args = parser.parse_args()
    release_version = check_release()
    abis = args.abis.split(",")
    if any(abi not in ABIS for abi in abis):
        parser.error("Unknown ABI; choose from " + ", ".join(ABIS))
    destination = ROOT / ".payload" / ("dev" if args.dev else "windows")
    # A prior --abis subset or dependency version must not leak into this build.
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True, exist_ok=True)
    ndk = find_ndk(args.ndk)
    build_relays(ndk, abis, destination)
    if args.dev:
        env = os.environ.copy()
        env.pop("TRANSBOX_BUNDLE", None)
        run(["cargo", "build", "--locked", "-p", "transbox"], env=env)
        print("Run: target/debug/transbox --runtime .payload/dev --adb /usr/bin/adb --browser /usr/bin/chromium")
        return
    prepare_windows(destination)
    collect_notices(destination)
    linker = windows_linker()
    collect_toolchain_notices(destination, linker)
    bundle = ROOT / ".payload/windows.zip"
    print("Compressing Windows runtime…", flush=True)
    create_zip(destination, bundle)
    run(["rustup", "target", "add", "x86_64-pc-windows-gnullvm"])
    env = os.environ.copy()
    env["CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER"] = linker
    env["PATH"] = str(Path(linker).parent) + os.pathsep + env["PATH"]
    env["TRANSBOX_BUNDLE"] = str(bundle)
    run(["cargo", "build", "--locked", "--release", "-p", "transbox", "--target", "x86_64-pc-windows-gnullvm"], env=env)
    release_assets(destination, release_version, ndk, abis, linker)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError, subprocess.CalledProcessError, OSError) as error:
        sys.exit(str(error))
