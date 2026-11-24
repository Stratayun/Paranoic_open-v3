# paranoic-server — own WebDAV server (roadmap 3.5)

A small Rust WebDAV server for a folder on a machine you control: backup/sync
target for the phone, Cryptomator vault host (only ciphertext arrives), and
the reference server for block 3 interoperability checks. Any WebDAV client
works (Paranoic, Cryptomator desktop/mobile, file managers).

```
paranoic-server token          # prints a new token (give to clients) and its BLAKE3 hash
paranoic-server run config.json
```

```json
{ "root": "/srv/paranoic", "listen": "0.0.0.0:8443", "username": "me",
  "token_blake3": "<64 hex>", "tls_cert": "/etc/paranoic/cert.pem",
  "tls_key": "/etc/paranoic/key.pem", "read_only": false }
```

Guarantees beyond plain WebDAV:
- MOVE/COPY never replace an existing entry (Linux: `renameat2 NOREPLACE`,
  copy via `create_new`); PUT with `If-None-Match: *` is create-new (O_EXCL).
- DELETE moves to `.paranoic-trash/<time>-<id>/` inside the root; the trash
  is invisible and inaccessible over WebDAV. Empty it on the host.
- Auth: random 256-bit token as Basic password (or Bearer); only its BLAKE3
  hash is stored; constant-time compare; failures delayed 0.5 s.
- TLS is mandatory unless `listen` is a loopback address; `read_only` limits
  methods to read-only WebDAV.

Built on dav-server 0.8.0 (Apache-2.0; newer versions need Rust 1.88).
Not included: multi-user accounts, quotas, rate limiting per IP, automatic
certificates (use a reverse proxy or certbot), trash expiry. Non-Linux hosts
have a residual check-then-rename race for MOVE. Run on Linux by the format 1.8
vault test (`paranoic-formats/tests/paranoic_vault.rs`): PROPFIND, PUT, MKCOL,
conditional MOVE/DELETE with trash; a full block 3 check is still pending.
