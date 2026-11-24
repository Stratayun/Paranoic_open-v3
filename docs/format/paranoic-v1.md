# Paranoic encrypted folder format, version 1 (EXPERIMENTAL)

Version manifest (§9) extends v1 through header flag bit 1.

Status: draft, not audited. Until an external audit this format is marked
experimental in the application (roadmap 1.8). Sections marked **[cloud]** are
deliberately open until the owner decides how the cloud is used; they do not
change anything below them, and the format version field lets them be added
without breaking existing vaults.

## 1. Purpose and threat model

A *vault* is a folder whose files and folder tree are stored as encrypted
objects inside an ordinary directory: a local folder, or a cloud folder
**[cloud]**. Each plaintext file is one object; there is no large container.

Adversary: anyone who can read, modify, delete, reorder, replay or add objects
in the storage (a cloud provider, a thief of the disk, another app with
storage access) but does not know the password and does not control the device
while the vault is unlocked.

**Guaranteed** (cryptographic):
- confidentiality of file contents, file and folder names, timestamps and exact
  file sizes inside a padding bucket;
- integrity of every file: any modification, truncation, extension, chunk
  reordering or chunk exchange between files is detected and the file is not
  returned;
- binding of a file to its vault, folder and name: moving or swapping objects
  between names, folders or vaults is detected;
- integrity of the vault header: parameters (KDF cost, padding) cannot be
  silently downgraded;
- the password is the only secret needed on a new device; the master key never
  leaves the device unencrypted.

**Made harder** (not guaranteed):
- folder structure: the number of objects per folder and the folder fan-out are
  visible; names are fixed-length, so name lengths are not;
- file sizes: visible up to the padding bucket (≤ 12 % overhead) when padding
  is on; exact when off;
- rollback of a single file to an older version, and a deleted entry put
  back: detected by the version manifest (§9, mandatory, header flag bit 1).

**Out of scope**:
- a compromised device (malware, keyloggers, memory dumps while unlocked);
- weak passwords beyond what Argon2id cost and the strength check provide;
- access patterns and timing (which objects are read and when);
- deletion of objects or of the whole vault (availability);
- rollback of the entire vault to an older consistent state on a device that
  has never opened a newer state (§9: the anchor is local to each device);
- copies the user exported or decrypted elsewhere.

## 2. Primitives

| Use | Primitive |
| --- | --- |
| Password → key-encryption key | Argon2id v1.3 (RFC 9106), 32-byte output |
| Subkeys | HKDF-SHA256 (RFC 5869) |
| Object encryption | XChaCha20-Poly1305 (draft-irtf-cfrg-xchacha) |
| Object names, folder paths | HMAC-SHA256 (RFC 2104), truncated |
| Header authentication | HMAC-SHA256 |
| Randomness | OS CSPRNG only |

No deterministic AEAD is used: names are not encrypted into the object name at
all (see §5), so AES-SIV is not needed. Fewer primitives, less to audit.

## 3. Keys

```
password ──Argon2id(salt, m, t, p)──► KEK ──XChaCha20-Poly1305──► master key K (32 random bytes)
recovery key (optional, 32 random bytes) ─HKDF─► KEK' ──► K  (second slot)

K ─HKDF-SHA256(salt = vault_id, info)─►
    "paranoic/v1/header-mac"   K_hmac   vault header authentication
    "paranoic/v1/file-header"  K_fh     file/folder header encryption
    "paranoic/v1/name"         K_name   object names
    "paranoic/v1/folder"       K_dir    folder storage paths
file key FK: 32 random bytes per file version, stored in its encrypted header
```

- **Password change** rewrites only the password slot; contents are untouched.
  This does NOT revoke an old password: a copy of the old header still opens the
  vault with the old password (cf. issue У-2). The format therefore defines
  **master-key rotation** as a separate, explicit operation: the whole tree
  is re-encrypted into a NEW vault (new vault_id and keys); each file is
  decrypted, verified and re-encrypted on the fly without plaintext on disk;
  the old vault is not modified and is deleted by the user after checking the
  new one. A vault with damaged objects is not rotated (they would be lost).
  The application must present password change and rotation as different
  actions.
- **Recovery key** (optional slot): 32 random bytes shown once, encoded as
  base32 in groups; KEK' = HKDF(recovery, salt = vault_id, "paranoic/v1/recovery").
- Removing a file's header (or rewriting the file) destroys its FK: per-file
  cryptographic deletion as far as the storage really deletes.

## 4. Vault header: `paranoic.vault`

Binary, little-endian, fixed field order:

```
magic        8   "PRNVLT01"
version      2   1
vault_id    16   random
flags        2   bit0 = size padding enabled; bit1 = version manifest
                 required (§9); other bits must be 0
argon_m      4   KiB, 65 536 ≤ m ≤ 1 048 576
argon_t      4   1 ≤ t ≤ 10
argon_p      4   1 ≤ p ≤ 4
salt        32   random
slot_count   1   1..=4
slots            slot_count × 74 bytes:
   type      1   1 = password, 2 = recovery key
   nonce    24
   sealed   48   XChaCha20-Poly1305(KEK, nonce, K, AD = magic‖version‖vault_id‖type)
   reserved  1   0
header_mac  32   HMAC-SHA256(K_hmac, every preceding byte)
```

Parsers reject unknown versions, unknown flag bits, out-of-range KDF
parameters (a hostile header must not make the device derive a 64 GiB key),
and any trailing bytes. After unwrapping K the MAC is verified **before** any
other field is trusted. KDF admission goes through the process-wide KDF gate.

Defaults: m = 256 MiB, t = 3, p = 1 on desktop; m = 128 MiB on low-memory
phones (header records what was used).

Android's default KDF admission policy allows one derivation and at most
256 MiB at a time, including imported vaults. Higher costs return a resource
limit error before Argon2 runs. Desktop defaults remain two derivations within
a shared 1 GiB budget. These are host resource limits, not changes to the format.

## 5. Layout

Flat storage with folder IDs. The plaintext tree is not mirrored.

```
paranoic.vault
d/<aa>/<30 chars>/           one storage folder per plaintext folder
    <26 chars>.f             a file entry
    <26 chars>.d             a subfolder entry (points to the child's folder ID)
```

- Every plaintext folder has a random 16-byte `folder_id`; the root uses
  `vault_id`. Its storage folder is
  `d/` + first 2 and next 30 characters of base32(HMAC-SHA256(K_dir, folder_id)).
- An entry's object name is base32(HMAC-SHA256(K_name, folder_id ‖ NFC(name)))
  truncated to 26 characters (130 bits), plus `.f` or `.d`. Base32 is lowercase
  RFC 4648 without padding: case-insensitive storages cannot collide entries.
- Names are Unicode-NFC normalised before hashing; the canonical NFC name is
  kept in the encrypted header. Two names that differ only in case are distinct
  in the vault; conflicts on case-insensitive targets are handled on export.
- Renaming or moving a folder rewrites only its `.d` entry (O(1) on cloud).
- **[cloud]** whether a per-folder encrypted index is kept to make listing one
  request instead of one header read per entry.

## 6. Entry header (both `.f` and `.d`)

```
magic        4   "PRNE"
version      1   1
kind         1   1 = file, 2 = folder
nonce       24
sealed     344   XChaCha20-Poly1305(K_fh, nonce, payload, AD = vault_id ‖ folder_id ‖ kind ‖ object name)
```

Payload (328 bytes, fixed):

```
name_len     2   ≤ 255 (UTF-8 bytes, NFC as given by the user)
name       256   zero-padded
modified     8   ms since epoch
file:   size 8 (plaintext length) ‖ FK 32 ‖ file_id 16 (random per file version) ‖ padded 1 (0/1) ‖ reserved 5
folder: child folder_id 16 ‖ reserved 46
```

The reader recomputes the object name from the decrypted name and the
expected `folder_id` and rejects the entry on mismatch: an entry copied to
another name or folder fails even though its AEAD tag is valid (the AD already
binds it; the recomputation also catches a different NFC form).

## 7. File contents (STREAM)

After the 374-byte header, the content is split into 64 KiB plaintext chunks,
each sealed with XChaCha20-Poly1305 under FK:

```
nonce_i = file_id[0..15] ‖ i (u64 big-endian) ‖ last (1 byte: 1 for the final chunk, else 0)
AD_i    = vault_id ‖ file_id
```

- Every file has exactly one final chunk; an empty file is one empty final
  chunk. Truncation at a chunk boundary, appended chunks and reordering fail.
- Padding is recorded per file (`padded`), so changing the vault flag later
  never breaks existing files. With padding on, the plaintext stream is `data ‖ zeros` up to
  Padmé(size) (Nikitin et al., PURBs): at most ~12 % overhead, the true size is
  in the header, and the reader verifies the padding is all zeros.
- A new file version uses a new FK and file_id; nonces never repeat under a key.
- Chunks are bound to the file (FK, file_id), the header to its folder and name.
  Rename and move therefore re-issue only the 374-byte header (same FK, file_id,
  size) and copy the encrypted chunks byte for byte, without re-encryption.
  Moving chunks into another file still fails (different FK and file_id).
- **[cloud]** Rename/move costs one object copy of the file's size. An O(1)
  rename (separate header and content objects, or provider-side copy) depends
  on the cloud decision.

## 8. Writing and crash safety

Every object is written to a temporary name and published by a create-new or
version-checked replace through the platform adapter (Core staged writes);
an interrupted write leaves the old version or a removable temporary object,
never a partially written entry. Folder creation publishes the storage folder
before its `.d` entry. **[cloud]** conditional replace (ETag/revision) and
conflict copies.

## 9. Version manifest (format v1 + flag bit 1)

Owner decision 2026-11-13 (Р-2). Status: implemented (reads, all writes, settling, per-device anchor; vaults without the manifest refused); fuzz target `v1_manifest`.

**Goal.** Detect (a) a single entry replaced by an older authentic version of
itself, (b) a deleted entry put back, (c) the whole vault returned to an older
state on a device that has already seen a newer one.

**Object** `paranoic.manifest` next to `paranoic.vault`:

```
magic        8   "PRNMAN01"
nonce       24
sealed       *   XChaCha20-Poly1305(K_man, nonce, body, AD = vault_id ‖ "paranoic/v1/manifest")
body:
  sequence   8   u64, +1 on every published change
  count      4   number of records (sorted by locator, no duplicates)
  records        count × 49 bytes:
    locator 16   first 16 bytes of HMAC-SHA256(K_man, folder storage path ‖ "/" ‖ object name)
    state    1   0 = present, 1 = present (change in flight), 2 = deleted (in flight)
    current 16   first 16 bytes of BLAKE3(entry header, 374 bytes)
    previous 16  state 1: the hash still accepted until settled; else zero
  pending    4   number of records in flight, then for each (sorted):
    locator 16, path_len 1, path (object path below the vault root, ≤ 96 bytes)
```

K_man = HKDF(K, salt = vault_id, "paranoic/v1/manifest"). The header of an
entry identifies its whole version: a new version has a new random FK and
file_id, so the hash of the 374-byte header changes, and its chunks are bound
to it (§7).

**Reading** (vault with bit 1): an entry is accepted only if its locator is in
the manifest and the hash of its header equals `current` (or `previous` while
state is 1). An object whose locator is absent, or whose record is `deleted`,
is reported as damaged, never listed or read. A record whose object is missing
is reported as missing (deletion stays out of scope, §1).

**Anchor.** Each device keeps, outside the vault and never synced, the highest
`sequence` it has seen per vault_id. Opening a manifest with a lower sequence
fails as "vault rolled back"; a higher one raises the anchor. A device that
never saw the newer state cannot detect a whole-vault rollback (§1).

**Writing.** Every change is planned as: (1) replace the manifest (sequence + 1;
the changed records in state 1 or 2 with the old hash kept as `previous`, and
their object paths), version-checked against the manifest that was read;
(2) the object operations of §8; (3) once they succeeded, a second manifest
replacement settles those records (state 0; settled deletions dropped), so the
old version is refused again at once. Only a crash between (1) and (3) leaves
the old version acceptable; the next change settles it against what the
storage holds, and a state not explained by the plan is left refused. Two
devices writing concurrently: a manifest replace fails as changed; that device
re-reads the manifest and plans again (no overwrite).

**Downgrade.** Bit 1 is in the MACed header, so the manifest cannot be removed
to fall back to unprotected reading; readers that do not know bit 1 reject the
vault (§4). The manifest is mandatory: no released build ever created a vault
without it, so a header without bit 1 (a pre-release test build) is refused,
not read unprotected and not upgraded.

**Cost.** Two manifest replacements per change (49 bytes per entry: ~0.5 MB for
10 000 entries); on a cloud this is an extra upload per change. Concurrent
writers from several devices conflict instead of merging.

## 10. Test vectors and fuzzing

- `docs/format/vectors/v1/vectors.json`: produced with seeded randomness
  (BLAKE3 XOF of a fixed seed; test-only, never in production) and the minimum
  KDF cost: vault header with a known password, root folder path, a folder
  entry, and files of 0, 1, 65 536 and 65 537 bytes plus a non-ASCII name, with
  padding on and off. The test regenerates them and requires byte equality, and
  independently decrypts the stored bytes with only the password.
- `docs/format/vectors/v1/manifest-vectors.json` (added 2026-11-14 after the
  first format audit): a header with flag bit 1 (manifest mandatory), a sealed
  manifest (sequence 3) recording the current version of one file, and two
  authentic versions of that file. Opening with only the password and the spec,
  the current version must be accepted and the older one refused (`Mismatch`):
  both decrypt, only the manifest tells them apart. The vectors above
  (`vectors.json`) predate the mandatory manifest and are kept unchanged.
- Rollback tests through Core: single-file rollback, resurrection, removal,
  rename of a stale source, anchor pinned right after a write, whole-vault
  rollback detected by the device anchor, and a rotation refused when a
  recorded object is missing.
- Negative cases (tests): every header byte flipped, truncation at every chunk
  boundary, chunk swap/reorder, chunks from another file, object moved to
  another name/folder/vault, non-zero padding, trailing bytes, unknown
  version/flags, oversized KDF; 3 000 random file corruptions and 150 header
  corruptions are never accepted.
- cargo-fuzz targets: vault header, entry header, stream, round trip, manifest (`fuzz/`).

## 11. Compared with Cryptomator vault 8

Kept: per-file objects, folder IDs with flat storage, per-file content keys,
names bound to the parent folder. Changed: names are not encrypted into the
object name (no name-length leak, no long-name shortening files), the content
AEAD covers a final-chunk flag (truncation detected), the header is MACed
(no parameter downgrade), KDF parameters are bounded, optional size padding,
explicit master-key rotation separate from password change.
