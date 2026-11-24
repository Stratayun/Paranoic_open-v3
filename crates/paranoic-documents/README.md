# Archives and text documents

Code implementation of roadmap 2.1–2.7 (Rust side). UI/FFI are separate.
Verification (Linux x86_64): see "Verification" below; tests `tests/block2.rs`,
`tests/block2_fixes.rs`, `tests/block2_fixtures.rs`.

## TXT and Markdown

`TextDocument::open` / `open_location` read bounded UTF-8 or BOM-marked UTF-16.
Malformed encoding and binary NUL are rejected. UTF-16 decoding, raw buffers and
owned document text use zeroizing allocations. UTF-8 edits check character
boundaries and expected revision. The configured limit is at most 64 MiB;
decoding/editing can temporarily hold several bounded allocations.

`save` preserves encoding, BOM and line endings through a host transactional
StreamSink (including the host's chosen conflict/replace policy). `save_as`
provides a concrete no-replace Storage adapter. `prepare_save` writes a private
spool and returns a PreparedArtifact only after spool commit. Its `submit_new`
publishes that exact artifact through Core::CopyExpected, retaining the source.
The journal contains digest/length and locations rather than plaintext content.
Existing-file replacement requires the host's transactional sink and exclusion
of concurrent writers; metadata comparisons do not implement compare-and-swap.
Markdown is text; this module executes no HTML, scripts or remote resources.

## ZIP / ZIP64 / AES

`ZipService::open` verifies EOCD/ZIP64, central-directory record boundaries,
entry count, index bytes and offsets before zip-rs allocates an index. Split
archives, invalid records, unsafe paths, duplicate/case-folded paths, Unix
symlinks/special files and file/directory prefix collisions are rejected. This
strict profile does not promise support for SFX or arbitrary trailing records.
Names must be UTF-8 and portable ResourcePaths. Entry metadata is available
through `entries`.

The default compressed input ceiling is 1 TiB, entry size 16 GiB, total output
64 GiB, 10000 entries, depth 64 and central directory 32 MiB. Caller settings can
adjust these within the declared API bounds; directory metadata has a hard
128 MiB cap and entry count a hard 100000 cap. Index structures additionally
consume bounded per-entry memory. This is not an allocator-level memory quota.
Payload buffers remain 64 KiB. Nested archives are never recursively expanded.

`extract` creates a new exclusive Files destination and stages every file before
publication. Declared output sizes/ratio are checked before mutation; actual
output bytes and exact EOF are checked while decoding. Reading to EOF lets the
codec validate CRC/AES authentication before the current file is published.
Interrupted or failed extraction retains already completed files and reports
NeedsAttention. No recursive rollback or automatic replay occurs. `extract`
runs synchronously with cooperative cancellation/progress. For a persisted,
recoverable operation use the journaled extraction job below. Hosts must serialize access and preserve the seekable source
against edits, and must not grant the destination inside Private Vault.

`create` writes into a new empty private seekable output and enforces output
write/seek limits. `create_prepared` owns an ArchiveOutput, explicitly aborts on
preparation failure, hashes the finished spool and commits it. A successful
PreparedArtifact can then be published through the durable Core queue with
`submit_new`; an unknown commit result is reported as NeedsAttention. Implement
ArchiveOutput using the host's private temporary-file system, with cleanup on
Drop. Never supply a live destination or an output already containing data.
ZIP64 file sizes and AES-256 file payload encryption are supported; names and
central-directory metadata remain visible. Password buffers are zeroized.

Persist the Core job ID before submission. After a lost response, query/reuse
that same request and ID. A committed job is historical evidence; inspect current
bytes separately. This module never deletes an original source automatically.

## Extraction as a Core job

`ZipService::prepare_extraction` / `SevenZipService::prepare_extraction` decode
into a NEW directory on a private spool grant registered with Core and return a
`PreparedExtraction` manifest (relative paths, lengths, BLAKE3; no passwords or
content). Failed preparation removes only that spool directory; an uncertain
cleanup is NeedsAttention. `submit_new` enqueues Core `MoveDirectoryAcross`
(spool → destination): journaled, create-new (never merged), every file read
back before the spool is removed, no automatic re-run after an uncertain result.
Persist the job ID first; after a lost reply query `Core::job`. If the destination
already exists the job fails and the spool stays for a retry to another name.
`verify_published` re-reads the destination through Core and requires an exact
match with the manifest (Integrity otherwise). The spool holds plaintext: it must
be an application-private grant, never Private Vault, cleared on Quick Lock.

## Verification

Owner command "ПРОВЕРИТЬ БЛОК 2" (2.1–2.7, Rust side), 2026-11-12, Linux x86_64,
rustc 1.87.0 and stable 1.97.0: 22 passed (block2 10, block2_fixes 7,
block2_fixtures 5), clippy without errors. Fixtures come from independent
producers: `tools/interop/block2/make_fixtures.py <dir>` (python-docx, openpyxl,
odfpy, LibreOffice 24, reportlab, Pillow, ffmpeg), then
`PARANOIC_BLOCK2_FIXTURES=<dir> PARANOIC_PDFIUM=<libpdfium.so>` with a worker
built by `cargo build -p paranoic-documents --features pdf --bin paranoic-pdf-worker`
(PDFium: bblanchon/pdfium-binaries chromium/8125 linux-x64). Without the
variables those tests are skipped. The live PDF worker is checked through /proc:
seccomp filter mode, no_new_privs, RLIMIT_FSIZE 0, RLIMIT_CPU 120 s, no worker
left after a session or a failed open. Defects found and fixed: the 7z header
pre-check rejected SubStreamsInfo, i.e. every archive written by sevenz-rust2;
DOCX lists defined in the paragraph style (Word/python-docx "List Bullet") and
numId 0 were misreported. Tests were checked by mutations (dictionary/substream
bounds, DTD, cell budget, style lists, mtime in the thumbnail key, no sandbox).

Earlier, 2.1–2.4 only: owner command "ПРОВЕРИТЬ БЛОК 2" (2.1–2.4), 2026-11-12, Linux x86_64, rustc
1.87.0 and stable 1.97.0: `cargo test -p paranoic-documents -- --test-threads=1`
10 passed; clippy clean for this crate. Interop: 7-Zip 23.01, Info-ZIP zip 3.0 /
unzip 6.0 (UTF-8 locale). Covered: traversal/absolute/drive/backslash/reserved
names, duplicates and case folds, file/dir prefix conflicts, symlink and device
entries, non-UTF-8 names, lying sizes, CRC/AES-HMAC corruption, truncation,
split EOCD, bad offsets, ratio/entry/total/index/depth/output limits, wrong
passwords, ZIP64 (`zip -fz`), Store/Deflate, 7z LZMA/LZMA2/copy/solid/AES with
encrypted headers, 7z traversal/duplicate/symlink/oversized dictionary/lying
size/payload CRC. Defect fixed: sevenz-rust2 0.7.0 writes kAnti inverted, so
created 7z directories were deletion markers; creation now avoids it and re-reads
the archive, failing closed. Not covered: Android/Windows, decoder memory
isolation (see 7z caveat), multi-GiB real ZIP64 payloads.

## 7z (2.2)

RAR is not supported: removed by owner decision (2026-11-12), together with the
native UnRAR dependency and its license obligations.

`SevenZipService::open/entries/extract` handles 7z through sevenz-rust2 0.7.0,
including password-protected input. `SevenZipService::create` streams independent
LZMA2 entries into a new private seekable output. This creation API produces
unencrypted 7z; encrypted creation is not claimed. Caller publishes only after
success and discards tentative output on error. All source lengths are checked.

The decoder uses portable-path/prefix-conflict checks, entry/total-byte/depth
limits and Files storage confinement. Extraction reserves a new root, stages
files and waits for decoder success (including final integrity checks) before
publishing. Pending writers are explicitly aborted on decode failure; empty
created directories may remain. Partial publication remains NeedsAttention, with
no automatic retry or recursive rollback. Cancellation is cooperative. Hosts serialize all writers.

CodecLimits caps pending file writers at 128 (including OS descriptor pressure).
SevenZ payload LZMA/LZMA2 dictionaries are
capped at 64 MiB. The 7z library parses/decodes headers before our entry/dictionary
checks: limiting encoded header bytes does not impose a hard bound on decoded
header allocations or all decoder CPU/memory. Do not treat these services as an
OS resource sandbox; hostile-input/resource testing and decoder isolation remain
verification/platform work. Password copies held internally by codecs are subject
to those codecs' memory lifecycle, not the wrapper's Zeroizing guarantee.

`create` re-reads the finished archive and requires exactly the requested
entries without anti-items (output must be Read + Write + Seek). No UI/FFI;
Android/Windows are not confirmed.

## DOCX / XLSX / ODS viewing (2.6)

`open_office` / `open_office_location` return a read-only structural view:
DOCX paragraphs (style, heading level from styles.xml, list flag, bold/italic/
underline runs) and tables; XLSX/ODS sheets as a bounded grid of display
strings (XLSX shared/inline strings, booleans, numbers, dates/times from
built-in or custom number formats, 1900/1904 systems; ODS formatted cell text
with typed-value fallback). Formulas are not evaluated (cached values shown),
macros/scripts/OLE/images/external links are ignored, encrypted packages are
Unsupported. Parsers are own code on quick-xml; type detection uses package
relationships and content types (OOXML) or `mimetype` (ODF).

Hostile-input design: strict ZIP preflight; only `.xml`, `.rels` and `mimetype`
parts are inflated, each with exact length and CRC, per-part and total limits;
UTF-8 XML only; DOCTYPE/DTD rejected, only predefined and character entities;
XML depth limit; sparse addresses and repeat attributes are clamped to the view
window (rows, columns, sheets, cell bytes, total text bytes) and reported as
`truncated`. View strings are zeroized on drop; parser-internal copies are not.
Code only: no tests were run for 2.6 (owner rule: verification on command).

## Photo, audio, video (2.7, Rust side)

`sniff` classifies by magic bytes (PNG/JPEG/GIF/WebP/BMP/HEIC/AVIF, MP3/AAC/
FLAC/OGG/WAV/M4A, MP4/MOV/3GP/MKV/WebM/AVI/ASF). `decode_image` (viewer, RGBA8,
zeroized) and `thumbnail_image` (PNG ≤ 1024 px) decode PNG/JPEG/GIF/WebP/BMP in
pure Rust with width/height/allocation ceilings, apply EXIF orientation and
strip all metadata by re-encoding. `thumbnail_rgba` accepts a frame decoded by
the platform (video thumbnails, HEIC/AVIF) so it uses the same cache.

`ThumbnailCache::ephemeral(PrivateDirectory)` keeps thumbnails only as
XChaCha20-Poly1305 envelopes; object names are a keyed BLAKE3 of storage id,
path, length, mtime and size, so neither pixels nor paths reach disk. Keys live
only in memory: `lock()` (Quick Lock, exit) drops them and removes this
session's objects; crash leftovers are ciphertext under a lost key. Failed
authentication evicts the entry. Private Vault must use a separate instance and
directory. `cached_thumbnail` reads through Core with length/metadata checks.

`MediaStream` gives platform players random-access `read_at` straight from a
Core grant (no plaintext temp file). Identity is pinned at open; reopen with a
different identity is SourceChanged. `ReadHandle::seek_to` (new, optional;
implemented by LocalStorage) avoids reopen-and-skip.

Platform playback exists in `ui/` (Android Media3 player over `MediaStream`,
recreated after background; desktop loopback stream). The thumbnail cache is
bounded (2000 entries / 64 MiB, oldest evicted) and its dedicated directory is
cleared of previous-session ciphertext on creation. It is not yet wired to the
file list UI.

## Review fixes (block 2, owner-commanded fix pass)

- Extraction cleanup removes only objects this extraction created (recorded
  files of the recorded length, recorded empty directories, the root only if
  created here); unexpected content → NeedsAttention, never recursive.
- `submit_new` uses Core `MoveDirectoryExpected`: the manifest is journaled
  and the copied tree must match it exactly before the spool is removed.
- 7z: the next header is CRC-checked and an encoded header's declared unpacked
  size and LZMA/LZMA2 dictionary are bounded (`sevenz_guard`) before
  sevenz-rust2 decodes anything.
- Office: `OfficeLimits.cells` bounds all materialized row/cell slots.
- PDF worker: one-slot reply channel, one deadline over write and reply,
  unsolicited replies and oversize PNG (IHDR) kill the worker; the worker
  confines itself on Linux/Android after loading PDFium and before parsing
  (seccomp denylist: files, network, processes, threads, exec, ptrace;
  rlimits: 1 GiB data, 120 s CPU, no file growth). Windows confinement
  (job object / restricted token) and an Android isolatedProcess service are
  still open. The UI releases the worker when the viewer closes or the app pauses.
- Text "Save": `PreparedArtifact::submit_replace` with
  `TextDocument::opened_metadata()` → Core `ReplaceExpected` replaces only the
  version that was opened, only with the prepared bytes.
