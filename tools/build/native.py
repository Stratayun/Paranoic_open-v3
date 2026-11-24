#!/usr/bin/env python3
"""Explicit native build/code generation entry point. Never runs tests.
Invoked by Gradle compilation/package tasks, or manually after owner authorizes a build.
Requires Rust 1.87+, JDK 17, Android NDK + cargo-ndk for Android targets.
PDFium (B3): a pinned bblanchon/pdfium-binaries release (chromium/8125, built
without V8/XFA) is downloaded once, checked against the SHA-256 recorded below
BEFORE it is opened, and only the library and its license notices are taken.
A library the owner put into ui/composeApp/native-libs wins; `--pdfium none`
skips PDFium (PDF then reports an unsupported capability).
"""
import argparse
import hashlib
import os
import pathlib
import platform
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "ui/composeApp/build"
CARGO = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
if not CARGO.is_absolute():
    CARGO = ROOT / CARGO

PDFIUM_RELEASE = "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F8125/"
# Archive -> SHA-256, recorded 2026-11-14 from the chromium/8125 release
# (args.gn: pdf_enable_v8 = false, pdf_enable_xfa = false; Android
# default_min_sdk_version = 23, 16 KB LOAD alignment; Windows DLL needs only
# KERNEL32/ADVAPI32/GDI32/USER32). Changing the version means re-checking these.
PDFIUM = {
    "pdfium-win-x64.tgz": "9a964261760cff52570aabf038ab346d3a5034dd8b30d0d5fe8861235d356b8b",
    "pdfium-android-arm64.tgz": "0495e57163429937caaee1bdd9b07924a7098dbd51d1b97645d124e349f097b4",
    "pdfium-android-arm.tgz": "ac4ac5319ded88ba5d885747a6b2641e7f712c9797de0770465104abe4dbf7da",
    "pdfium-android-x64.tgz": "bd68707f29f2316c20f9d5c793b8731796a4a65408ac4d3c30bc606390354e41",
}
PDFIUM_ANDROID = {"arm64-v8a": "pdfium-android-arm64.tgz", "armeabi-v7a": "pdfium-android-arm.tgz", "x86_64": "pdfium-android-x64.tgz"}
PDFIUM_LIMIT = 64 << 20
OVERRIDES = ROOT / "ui/composeApp/native-libs"

def pdfium_archive(name):
    """Cached, hash-checked archive path. A mismatch is deleted and fails the build."""
    expected = PDFIUM[name]
    cache = OUT / "pdfium-cache"
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / name
    if not path.is_file():
        partial = cache / (name + ".part")
        with urllib.request.urlopen(PDFIUM_RELEASE + name, timeout=120) as response, open(partial, "wb") as out:
            total = 0
            while chunk := response.read(1 << 20):
                total += len(chunk)
                if total > PDFIUM_LIMIT:
                    raise SystemExit(f"{name}: download larger than expected")
                out.write(chunk)
        partial.replace(path)
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if digest != expected:
        path.unlink()
        raise SystemExit(f"{name}: SHA-256 {digest} does not match the pinned {expected}")
    return path

def pdfium_extract(name, member, destination, licenses):
    """Copy one regular file and the license notices out of a verified archive."""
    with tarfile.open(pdfium_archive(name), "r:gz") as archive:
        def regular(entry):
            info = archive.getmember(entry)
            if not info.isfile() or info.size > PDFIUM_LIMIT:
                raise SystemExit(f"{name}: {entry} is not a regular file")
            return archive.extractfile(info).read()
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(regular(member))
        licenses.mkdir(parents=True, exist_ok=True)
        for info in archive.getmembers():
            path = pathlib.PurePosixPath(info.name)
            notice = info.name == "LICENSE" or (len(path.parts) == 2 and path.parts[0] == "licenses")
            if notice and info.isfile() and ".." not in path.parts:
                (licenses / ("pdfium.txt" if info.name == "LICENSE" else path.name)).write_bytes(regular(info.name))

def run(*args):
    subprocess.run(list(map(str, args)), cwd=ROOT, check=True)

def library():
    system = platform.system()
    return "paranoic_bridge.dll" if system == "Windows" else "libparanoic_bridge.dylib" if system == "Darwin" else "libparanoic_bridge.so"

def bindings():
    run("cargo", "build", "--locked", "-p", "paranoic-bridge", "--profile", "bindings", "--features", "bindgen")
    output = OUT / "generated/uniffi"
    output.mkdir(parents=True, exist_ok=True)
    run("cargo", "run", "--locked", "-p", "paranoic-bridge", "--profile", "bindings", "--features", "bindgen", "--bin", "uniffi-bindgen",
        "--", "generate", "--library", CARGO / "bindings" / library(), "--crate", "paranoic_bridge", "--language", "kotlin",
        "--config", ROOT / "crates/paranoic-bridge/uniffi.toml", "--out-dir", output)
    expected = output / "io/github/stratayun/paranoic/nativecore/paranoic_bridge.kt"
    if not expected.is_file() or "class NativeRuntime" not in expected.read_text(encoding="utf-8"):
        raise SystemExit("UniFFI did not generate the expected nativecore NativeRuntime bindings")

WORKER = "paranoic-pdf-worker"

def desktop(pdfium="fetch"):
    run("cargo", "build", "-p", "paranoic-bridge", "--release")
    # PDFium runs only in this separate worker process, never in the app process.
    run("cargo", "build", "-p", "paranoic-documents", "--release", "--features", "pdf", "--bin", WORKER)
    system, machine = platform.system(), platform.machine().lower()
    architecture = "aarch64" if machine in ("aarch64", "arm64") else "x86-64"
    prefix = {"Windows": "win32", "Darwin": "darwin", "Linux": "linux"}[system] + "-" + architecture
    output = OUT / "native/resources" / prefix
    output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(CARGO / "release" / library(), output / library())
    # The worker is an executable, not a JVM resource: Compose app resources
    # install it next to the app (read-only install dir) and expose that
    # directory as compose.application.resources.dir, also for `run`.
    worker = WORKER + (".exe" if system == "Windows" else "")
    app_resources = OUT / "native/app-resources/common"
    app_resources.mkdir(parents=True, exist_ok=True)
    shutil.copy2(CARGO / "release" / worker, app_resources / worker)
    # PDFium (B3): Windows x64 only. JNA extracts the resource and Rust gets its
    # absolute path, never a PATH/working-directory lookup.
    dll = output / "pdfium.dll"
    if pdfium == "fetch" and prefix == "win32-x86-64" and not (OVERRIDES / "desktop" / prefix / "pdfium.dll").is_file():
        pdfium_extract("pdfium-win-x64.tgz", "bin/pdfium.dll", dll, app_resources / "licenses/pdfium")
    elif dll.exists():
        dll.unlink()

def android(abis, pdfium="fetch"):
    allowed = {"arm64-v8a", "armeabi-v7a", "x86_64"}
    selected = abis.split(",")
    if not selected or any(abi not in allowed for abi in selected):
        raise SystemExit("Unsupported Android ABI")
    args = ["cargo", "ndk"]
    for abi in selected:
        args.extend(("--target", abi))
    args.extend(("--platform", "26", "--output-dir", str(OUT / "native/android"), "build", "-p", "paranoic-bridge", "--release"))
    run(*args)
    # Worker executable, packaged as lib*.so so Android installs it into
    # nativeLibraryDir with exec permission (extractNativeLibs=true).
    triples = {"arm64-v8a": "aarch64-linux-android", "armeabi-v7a": "armv7-linux-androideabi", "x86_64": "x86_64-linux-android"}
    for abi in selected:
        run("cargo", "ndk", "--target", abi, "--platform", "26", "build", "-p", "paranoic-documents",
            "--release", "--features", "pdf", "--bin", WORKER)
        destination = OUT / "native/android" / abi
        destination.mkdir(parents=True, exist_ok=True)
        shutil.copy2(CARGO / triples[abi] / "release" / WORKER, destination / "libparanoic_pdf_worker.so")
        # PDFium (B3) next to the worker, unless the owner supplied one.
        library = destination / "libpdfium.so"
        if pdfium == "fetch" and not (OVERRIDES / "android" / abi / "libpdfium.so").is_file():
            pdfium_extract(PDFIUM_ANDROID[abi], "lib/libpdfium.so", library, OUT / "native/android-assets/licenses/pdfium")
        elif library.exists():
            library.unlink()

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("bindings", "desktop", "android"))
    parser.add_argument("--abis", default="arm64-v8a,x86_64")
    parser.add_argument("--pdfium", choices=("fetch", "none"), default="fetch")
    args = parser.parse_args()
    if args.action == "bindings":
        bindings()
    elif args.action == "desktop":
        desktop(args.pdfium)
    else:
        android(args.abis, args.pdfium)

