# Fuzzing (format 1.8, `paranoic_v1`)

Separate from the main workspace: needs nightly and cargo-fuzz.

```sh
cd fuzz
cargo +nightly run --release --example seed      # genuine objects as seed corpora
cargo +nightly fuzz run -O v1_vault_header -- -max_total_time=300
```

| Target | Input from an untrusted store | Property |
| --- | --- | --- |
| `v1_vault_header` | vault header | no panic; only the genuine header unlocks |
| `v1_entry_header` | entry header under an object name | no panic; only the genuine header at its own name opens |
| `v1_decrypt_file` | whole file object | exact plaintext + commit, or failure without commit |
| `v1_roundtrip` | names, sizes at chunk/padding edges, content | encrypt→decrypt equal; truncation/extension/bit flip detected; rename re-issue opens only under the new name |
