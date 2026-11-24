# Cloud providers (block 3)

Code for roadmap 3.1–3.9 (Rust side). Verification status (tests against the
own WebDAV server, live accounts not yet) and UI status — only in the root ROADMAP.

## API (3.1)

`CloudProvider`: quota, root, metadata, paged list, create folder, ranged
download stream, create-new upload, rename/move/trash with an expected
revision. `CloudId` is the provider's opaque id, never a path.
`CloudCapabilities` states what the server guarantees (atomic create-new,
conditional mutation, server SHA-256, resumable upload, duplicate names).

Shared rules: names are portable single components (same rules as local
paths); no entry is ever replaced; a write whose outcome is unknown returns
NeedsAttention and is never replayed automatically; folders are trashed only
when empty; nothing is deleted permanently. Bearer tokens go only to the API
host; pre-authenticated upload/download URLs are host-allowlisted and get no
token. Redirects are never followed automatically. Responses are capped at 16 MiB.

`upload_from_core` streams a Core file to a provider and proves the stored
bytes (server SHA-256, else full download readback); the source is kept.
`download_to_storage` stages into a new path and publishes only after the exact
length (and provider SHA-256 when available) matched.

## Providers

| | create-new | conditional mutation | hash | notes |
|---|---|---|---|---|
| Google Drive (3.2) | pre-check + post-check (duplicate names allowed) | re-check `version` before PATCH | SHA-256 | resumable 8 MiB chunks; Google Docs = Native, not downloadable |
| OneDrive (3.3) | server `conflictBehavior=fail` | `If-Match` eTag | readback | upload session 10 MiB chunks, cancelled on error |
| pCloud (3.4) | pre-check + `renameifexists=1`, renamed upload deleted → AlreadyExists | re-check hash/modified | SHA-256 via `checksumfile` (EU) else readback | regional host from login; whole-folder listings sliced into pages |

## OAuth and tokens (3.6)

`OAuthClient::{google, onedrive, pcloud}` → `begin()` (system browser URL,
PKCE S256 for Google/Microsoft, random state) → `finish()` with the redirect URL
(constant-time state check, exact redirect match). Windows: `LoopbackRedirect`
(127.0.0.1, random port, timeout/cancel). Android: the host receives the
custom-scheme/app-link redirect and passes it to `finish`. `CloudSession`
refreshes two minutes before expiry and once after a 401 (rotated tokens go to
the host `persist` callback); `lock()` forgets tokens in memory (Quick Lock).
`seal_tokens`/`open_tokens` bind provider + account in the AEAD context; the key
comes from the keyring session or platform keystore; `TokenStore` is the host's
app-private, backup-excluded persistence.

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
- `move_no_replace` only on Drive/OneDrive (atomic relocate); pCloud: Unsupported.
- Binding key = provider + account id + root id, so a grant id cannot be
  re-pointed at another account/folder.

## Backup and sync (3.8)

`SyncBaseline::plan` observes both trees (no side effects) and returns jobs;
`submit` re-checks each path right before submission and never resubmits a
journaled job id; `commit` advances the baseline only for succeeded jobs.
Modes: `Backup` (local is the source; cloud-only/cloud-modified files are only
reported) and `TwoWay`. Conflicts are never auto-resolved: the cloud version is
saved locally as a KeepBoth copy. Local files are never deleted by sync; cloud
deletions are opt-in (`trash_remote_deletions`) and go to the trash. Equal
content without a baseline (first run, crash) is adopted after SHA-256 or
QuickXorHash comparison; unknown equality = conflict. The baseline contains
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
`check_vault_support` fails early where writes are unsafe. Writable on Drive,
OneDrive and WebDAV, including RenameMapped/ReshapeDirectory with inspection
and recovery (`mapped.rs`: wrapper moved into hidden staging, mapping changed
without overwriting — new uploaded first, old kept as evidence — then one
atomic publish). pCloud: read-only vaults until live verification.

## Before first live use (blockers)

- Client registrations are required (Google Cloud console, Microsoft Entra,
  pCloud app) with the redirect URIs; ids/secrets are host configuration.
- pCloud: confirm against the live API that `uploadfile` (PUT) honours
  `renameifexists`/`nopartial`, and that `renamefile` cannot overwrite in the
  pre-checked case; until then pCloud writes are unverified.
- Google Drive create-new and all pCloud renames have a residual race with
  concurrent writers (documented, detected after the fact where possible).
- QuickXorHash must be checked against a known OneDrive vector.
- pCloud: `renamefolder` overwrite behaviour; file moves use copy(noover)+trash.
