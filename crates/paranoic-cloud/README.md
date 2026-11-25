# Remote storage over WebDAV (block 3)

Rust side of block 3: WebDAV client, cloud folders in the Core queue, backup/sync
and vaults on the own server. Status — only in the root ROADMAP.

## API (3.1)

`CloudProvider`: quota, root, metadata, paged list, create folder, ranged
download stream, create-new upload, rename/move/trash with an expected
revision. `CloudId` is the provider's opaque id, never a path.
`CloudCapabilities` states what the server guarantees (atomic create-new,
conditional mutation, server SHA-256, resumable upload, duplicate names).

Shared rules: names are portable single components (same rules as local
paths); no entry is ever replaced; a write whose outcome is unknown returns
NeedsAttention and is never replayed automatically; folders are trashed only
when empty; nothing is deleted permanently. Redirects are never followed automatically. Responses are capped at 16 MiB.

`upload_from_core` streams a Core file to a provider and proves the stored
bytes (server SHA-256, else full readback); the source is kept.
`download_to_storage` stages into a new path and publishes only after the exact
length (and server SHA-256 when available) matched.

## Providers

Only WebDAV: the owner's `paranoic-server` and other WebDAV servers
(Nextcloud, pCloud WebDAV, …). Google Drive, OneDrive and pCloud API adapters
with OAuth were removed by the owner's decision (25.11.2026): encrypted folders
are moved to such clouds by hand. The code remains in git history.

## Cloud folders in the Core queue (3.7)

`CloudStorage::open(id, provider, root_folder, spool_dir)` turns a cloud folder
into a Core `Storage` grant (register it in Core). All Core operations then work
local↔cloud and cloud↔cloud with the journal, progress, cancellation and
recovery: create folder, copy file/tree (Fail/Skip/KeepBoth/Replace), verified
copies, move across storages, delete (→ provider trash).

- Paths resolve by exact-name lookups; duplicate names are Unsupported (no guess).
- Listings are complete or fail (non-portable/duplicate names) because Core
  copies and deletes whole trees; ".paranoic-*" staging objects are hidden.
- `stage` spools to `spool_dir` (0700, app-private, backup-excluded) and uploads
  create-new on publish; `purge_spool` removes crash leftovers at startup.
- `publish_tree`/`stage_bundle` build a hidden ".paranoic-<job>" folder and
  expose it with one relocate; failures trash the staging folder.
- `move_no_replace` via WebDAV `MOVE` + `Overwrite: F`.
- Binding key = server + account + root, so a grant id cannot be
  re-pointed at another server/folder.

## Backup and sync (3.8)

`SyncBaseline::plan` observes both trees (no side effects) and returns jobs;
`submit` re-checks each path right before submission and never resubmits a
journaled job id; `commit` advances the baseline only for succeeded jobs.
Modes: `Backup` (local is the source; cloud-only/cloud-modified files are only
reported) and `TwoWay`. Conflicts are never auto-resolved: the cloud version is
saved locally as a KeepBoth copy. Local files are never deleted by sync; cloud
deletions are opt-in (`trash_remote_deletions`) and go to the trash. Equal
content without a baseline (first run, crash) is adopted after a server SHA-256
comparison; unknown equality = conflict. The baseline contains
names: persist it only via `seal`/`open` (AEAD bound to the pair).
Residual race: a change between `submit`'s check and job execution.

## WebDAV (3.5 client side)

`WebDav::new(http, "https://host/dav/", credentials, server_trash)` — own
Paranoic server, Nextcloud/ownCloud and other RFC 4918 servers. Create-new
uploads `PUT` + `If-None-Match: *`; folders `MKCOL`; rename/move `MOVE` +
`Overwrite: F` + `If-Match`; deletion `DELETE` + `If-Match` (trash on the own
server and Nextcloud). Nextcloud SHA-1 checksums are compared after upload;
otherwise `verify_remote` reads the file back. HTTP only to loopback; Basic
credentials never follow redirects. Paths are ids and change on rename.

## Cryptomator in the cloud (3.9)

The vault engine runs on any Core `Storage`, so a `CloudStorage` grant hosts a
vault with local encryption and only ciphertext in the cloud.
`check_vault_support` fails early where writes are unsafe. Writable on WebDAV, including RenameMapped/ReshapeDirectory with inspection
and recovery (`mapped.rs`: wrapper moved into hidden staging, mapping changed
without overwriting — new uploaded first, old kept as evidence — then one
atomic publish).
