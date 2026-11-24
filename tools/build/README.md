# Native build inputs

The only ABI is crates/paranoic-bridge / UniFFI 0.29.4. The Gradle project uses
Kotlin bindings generated from that exact library, with JNA 5.17.0. Checksums are
left enabled. Both generated Kotlin and native libraries are build outputs.

Required tools when the owner authorizes a build:
Rust 1.87+, Cargo, Python3, JDK17, Android SDK36 and NDK, cargo-ndk, and the Rust
targets matching the desired Android ABIs. Windows native packages/MSI are built
on Windows, using matching JDK/architecture and packaging tools.

The original implementation was prepared without Cargo/build execution.
The Kotlin CI repair on 2026-11-12 verified host bindings, desktop Core/PDF-worker
builds, Kotlin desktop/Android compilation and JVM unit tests on Linux.
See ROADMAP for the exact scope and commit; Android native packaging/device
execution and Windows runtime are separate checks. A changed manifest still
requires an updated lockfile before --locked checks.

## Bridge and bindings

From the repository root, after the owner permits building:

```sh
python tools/build/native.py bindings
python tools/build/native.py desktop
python tools/build/native.py android --abis arm64-v8a,x86_64
```

Gradle compilation/package tasks run these same build actions. Only the relevant
native package task is attached: desktopProcessResources builds desktop Core;
Android mergeNativeLibs builds Android Core. Set -Pparanoic.androidAbis=... to
choose supported ABIs, and -Pparanoic.python=... if Python has a different command.
Manual workflows are opt-in; no push/PR/schedule triggers are configured.

Bindings use the dedicated Cargo `bindings` profile (inherits release, keeps
symbols), not the stripped `release` library. UniFFI 0.29.4 reads ELF `.symtab`;
`strip = "symbols"` can otherwise produce zero components with exit code 0.
Generation selects `--crate paranoic_bridge` and requires the expected
`nativecore/paranoic_bridge.kt` with `NativeRuntime`. Missing metadata/output
must fail before Kotlin compilation. Packaged libraries still use release.
The generated directory is a declared Gradle output wired to both JVM leaves.

Manual Kotlin/device workflows are described in [tests/README.md](../../tests/README.md).
Actual validation evidence is recorded only in the root ROADMAP.

## Pdfium (bundled, B3)

This code pins pdfium-render 0.9.5 with Pdfium ABI 8125. `native.py desktop`
and `native.py android` take it from the bblanchon/pdfium-binaries release
`chromium/8125`: each archive is downloaded once into `build/pdfium-cache`,
its SHA-256 is compared with the value recorded in `native.py` **before** it is
opened (a mismatch deletes it and fails the build), and only the library plus
the license notices are copied out. Recorded properties of these archives:
`pdf_enable_v8 = false`, `pdf_enable_xfa = false`; Android
`default_min_sdk_version = 23` (app: 26) and 16 KiB LOAD alignment; the
Windows DLL imports only KERNEL32, ADVAPI32, GDI32 and USER32. A new version
means downloading, re-checking these properties and recording new hashes.
Nothing is downloaded at app runtime.

| Platform | Bundled as | Runtime resolution |
| --- | --- | --- |
| Android (arm64-v8a, armeabi-v7a, x86_64) | build/native/android/&lt;abi&gt;/libpdfium.so; notices in assets/licenses/pdfium | applicationInfo.nativeLibraryDir/libpdfium.so |
| Windows x64 | build/native/resources/win32-x86-64/pdfium.dll; notices installed under licenses/pdfium | Classpath resource extracted by JNA; the absolute path is passed to Rust. |
| Windows explicit override | An application-administered trusted location | -Dparanoic.pdfium=C:/trusted/path/pdfium.dll |

Owner override: a library placed in `ui/composeApp/native-libs/android/<abi>/libpdfium.so`
or `ui/composeApp/native-libs/desktop/win32-x86-64/pdfium.dll` (ignored by Git)
is used instead and nothing is downloaded for it. `-Pparanoic.pdfium=none`
(or `native.py … --pdfium none`) builds without Pdfium, e.g. offline; if no
resource/override is present, the fallback is %LOCALAPPDATA%/Paranoic/native/pdfium.dll,
and the app reports an unsupported PDF capability if that explicit library
cannot be loaded; it does not search the working directory. Native reader and
ordinary file operations do not require Pdfium.

Pdfium in-process parsing has no hard heap/CPU sandbox. Input/output/page limits
bound app-controlled buffers, not arbitrary native-parser work. JS/XFA/forms are
not invoked by this code; the native-build requirement above remains necessary.

## Media and licenses

Android Media3 reads Core ranges through a custom DataSource. OpenJFX 21.0.13
provides the Windows player (JDK17); its codec support is platform-dependent.
JavaFX has GPLv2 with Classpath Exception; use OpenJFX artifacts, not an Oracle
commercial runtime download. Pdfium uses its upstream BSD-style licenses and
third-party notices; pdfium-render is MIT/Apache-2.0. No VLCJ/GPL dependency was added.

Primary documentation:
- https://mozilla.github.io/uniffi-rs/0.29/
- https://docs.rs/crate/pdfium-render/0.9.5
- https://github.com/bblanchon/pdfium-binaries
- https://developer.android.com/media/media3/exoplayer/customization
- https://gluonhq.com/products/javafx/

