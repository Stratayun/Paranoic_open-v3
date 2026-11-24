//! Paranoic format v1: deterministic test vectors (spec §10) and robustness
//! against random corruption. Vectors live in docs/format/vectors/v1/;
//! regenerate only on a deliberate format change: PARANOIC_WRITE_VECTORS=1.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{cell::RefCell, io::Write, rc::Rc};
use paranoic_crypto::StreamSink;
use paranoic_formats::paranoic_v1::*;

/// Deterministic randomness for vectors ONLY: BLAKE3 XOF of a fixed seed.
struct Seeded(blake3::OutputReader);
impl Seeded { fn new(seed: &str) -> Self { Self(blake3::Hasher::new().update(seed.as_bytes()).finalize_xof()) } }
impl RandomSource for Seeded { fn fill(&mut self, out: &mut [u8]) -> paranoic_crypto::Result<()> { self.0.fill(out); Ok(()) } }
const KDF: KdfParams = KdfParams { memory_kib: 65_536, iterations: 1, lanes: 1 };
fn ok() -> paranoic_crypto::Result<()> { Ok(()) }
fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

#[derive(Default)] struct Out { data: Vec<u8>, committed: bool }
struct Sink(Rc<RefCell<Out>>, Vec<u8>);
impl Write for Sink { fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.1.extend_from_slice(b); Ok(b.len()) } fn flush(&mut self) -> std::io::Result<()> { Ok(()) } }
impl StreamSink for Sink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { let mut o = self.0.borrow_mut(); o.data = self.1.clone(); o.committed = true; Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { Ok(()) }
}

fn vectors() -> serde_json::Value {
    let mut cases = Vec::new();
    for padding in [false, true] {
        let mut rng = Seeded::new(if padding { "paranoic-v1-vectors-padded" } else { "paranoic-v1-vectors" });
        let (header, keys) = create_vault(b"vector password", KDF, padding, &mut rng, &mut ok).unwrap();
        let root = keys.vault_id;
        let mut files = Vec::new();
        for (name, size) in [("empty", 0usize), ("one", 1), ("chunk", 65_536), ("chunk+1", 65_537), ("Café NFC", 5)] {
            let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
            let mut out = Vec::new();
            keys.encrypt_file(&root, NewFile { name, modified_ms: 1_700_000_000_000, size: size as u64 }, data.as_slice(), &mut out, &mut rng, &mut |_| Ok(())).unwrap();
            files.push(serde_json::json!({ "name": name, "size": size, "object": keys.entry_name(&root, name, EntryKind::File).unwrap(),
                "plaintext_blake3": hex(blake3::hash(&data).as_bytes()), "ciphertext": if size <= 65_537 { hex(&out) } else { String::new() } }));
        }
        let child = [7u8; 16];
        let folder = keys.seal_folder(&root, "Folder", 1_700_000_000_000, &child, &mut rng).unwrap();
        cases.push(serde_json::json!({ "padding": padding, "password": "vector password",
            "kdf": { "memory_kib": KDF.memory_kib, "iterations": KDF.iterations, "lanes": KDF.lanes },
            "header": hex(&header), "root_folder_path": keys.folder_path(&root).unwrap(),
            "folder_entry": { "name": "Folder", "object": keys.entry_name(&root, "Folder", EntryKind::Folder).unwrap(), "header": hex(&folder), "child": hex(&child) },
            "files": files }));
    }
    serde_json::json!({ "format": "paranoic-v1", "note": "seeded randomness (BLAKE3 XOF); production uses the OS RNG", "cases": cases })
}

#[test] fn vectors_are_stable_and_decrypt() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/format/vectors/v1/vectors.json");
    let computed = vectors();
    if std::env::var("PARANOIC_WRITE_VECTORS").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&computed).unwrap() + "\n").unwrap();
    }
    let stored: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("vectors file")).unwrap();
    assert_eq!(stored, computed, "format output changed: vectors no longer match");
    // Independent direction: the stored bytes decrypt with only the password.
    let unhex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
    for case in stored["cases"].as_array().unwrap() {
        let keys = unlock(&unhex(case["header"].as_str().unwrap()), b"vector password", &mut ok).unwrap();
        let root = keys.vault_id;
        for file in case["files"].as_array().unwrap() {
            let out = Rc::new(RefCell::new(Out::default()));
            keys.decrypt_file(&root, file["object"].as_str().unwrap(), unhex(file["ciphertext"].as_str().unwrap()).as_slice(),
                Box::new(Sink(out.clone(), Vec::new())), u64::MAX, &mut |_| Ok(())).unwrap();
            assert_eq!(hex(blake3::hash(&out.borrow().data).as_bytes()), file["plaintext_blake3"].as_str().unwrap());
        }
    }
}

/// Manifest vectors (spec §9; audit of format 1.8, 2026-11-14): the stored
/// vectors above predate the mandatory manifest. Here: a header with flag
/// bit 1, a sealed manifest recording the CURRENT version of a file, and an
/// older authentic version of the same file. Accept the current one, refuse
/// the older one — with only the password and the spec.
fn manifest_vectors() -> serde_json::Value {
    use paranoic_formats::paranoic_manifest as man;
    let mut rng = Seeded::new("paranoic-v1-manifest-vectors");
    let (header, mut keys) = create_vault(b"vector password", KDF, false, &mut rng, &mut ok).unwrap();
    let header = require_manifest(&header, &keys).unwrap();
    keys.manifest_required = true;
    let root = keys.vault_id;
    let folder_path = keys.folder_path(&root).unwrap();
    let object = keys.entry_name(&root, "doc.txt", EntryKind::File).unwrap();
    let encrypt = |rng: &mut Seeded, body: &[u8]| {
        let mut out = Vec::new();
        keys.encrypt_file(&root, NewFile { name: "doc.txt", modified_ms: 1_700_000_000_000, size: body.len() as u64 }, body, &mut out, rng, &mut |_| Ok(())).unwrap();
        out
    };
    let older = encrypt(&mut rng, b"version one");
    let current = encrypt(&mut rng, b"version two!");
    let locator = man::locator(&keys, &folder_path, &object).unwrap();
    let mut manifest = man::Manifest::default();
    manifest.insert_present(locator, man::header_hash(&current[..ENTRY_HEADER_LEN]).unwrap()).unwrap();
    manifest.sequence = 3;
    let sealed = man::seal(&keys, &manifest, &mut rng).unwrap();
    serde_json::json!({ "format": "paranoic-v1 + manifest (spec §9)", "note": "seeded randomness (BLAKE3 XOF); production uses the OS RNG",
        "password": "vector password", "kdf": { "memory_kib": KDF.memory_kib, "iterations": KDF.iterations, "lanes": KDF.lanes },
        "header": hex(&header), "header_flags": 2, "root_folder_path": folder_path, "object": object,
        "manifest": hex(&sealed), "manifest_sequence": 3, "locator": hex(&locator),
        "current": { "ciphertext": hex(&current), "plaintext": "version two!", "verdict": "accepted" },
        "older": { "ciphertext": hex(&older), "plaintext": "version one", "verdict": "mismatch (older authentic version)" } })
}

#[test] fn manifest_vectors_are_stable_and_refuse_an_older_version() {
    use paranoic_formats::paranoic_manifest as man;
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/format/vectors/v1/manifest-vectors.json");
    let computed = manifest_vectors();
    if std::env::var("PARANOIC_WRITE_VECTORS").is_ok() {
        std::fs::write(&path, serde_json::to_string_pretty(&computed).unwrap() + "\n").unwrap();
    }
    let stored: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("manifest vectors file")).unwrap();
    assert_eq!(stored, computed, "format output changed: manifest vectors no longer match");
    let unhex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
    let keys = unlock(&unhex(stored["header"].as_str().unwrap()), b"vector password", &mut ok).unwrap();
    assert!(keys.manifest_required, "flag bit 1 not set");
    let manifest = man::open(&keys, &unhex(stored["manifest"].as_str().unwrap())).unwrap();
    assert_eq!(manifest.sequence, 3);
    let locator = man::locator(&keys, stored["root_folder_path"].as_str().unwrap(), stored["object"].as_str().unwrap()).unwrap();
    let verdict = |which: &str| {
        let bytes = unhex(stored[which]["ciphertext"].as_str().unwrap());
        manifest.check(&locator, &man::header_hash(&bytes[..ENTRY_HEADER_LEN]).unwrap())
    };
    assert_eq!(verdict("current"), man::Verdict::Accepted);
    assert_eq!(verdict("older"), man::Verdict::Mismatch, "an older authentic version is accepted");
    // Both versions are authentic: only the manifest tells them apart.
    for which in ["current", "older"] {
        let out = Rc::new(RefCell::new(Out::default()));
        keys.decrypt_file(&keys.vault_id, stored["object"].as_str().unwrap(), unhex(stored[which]["ciphertext"].as_str().unwrap()).as_slice(),
            Box::new(Sink(out.clone(), Vec::new())), u64::MAX, &mut |_| Ok(())).unwrap();
        assert_eq!(out.borrow().data, stored[which]["plaintext"].as_str().unwrap().as_bytes());
    }
}

/// Random corruption of headers, entries and streams: never a panic, never
/// accepted (fuzz-style; full cargo-fuzz targets are not set up).
#[test] fn random_corruption_is_never_accepted() {
    let (header, keys) = create_vault(b"pw for fuzz", KDF, true, &mut OsRandom, &mut ok).unwrap();
    let root = keys.vault_id;
    let object = keys.entry_name(&root, "f", EntryKind::File).unwrap();
    let mut file = Vec::new();
    let data = vec![9u8; 70_000];
    keys.encrypt_file(&root, NewFile { name: "f", modified_ms: 0, size: data.len() as u64 }, data.as_slice(), &mut file, &mut OsRandom, &mut |_| Ok(())).unwrap();
    let mut rng = Seeded::new("corruption");
    let mut pick = |n: usize| { let mut b = [0u8; 8]; rng.fill(&mut b).unwrap(); (u64::from_le_bytes(b) % n as u64) as usize };
    for round in 0..3000 {
        let mut bytes = file.clone();
        match round % 4 {
            0 => { let i = pick(bytes.len()); bytes[i] ^= 1 + (pick(255) as u8); }
            1 => { let cut = pick(bytes.len()); bytes.truncate(cut); }
            2 => { let i = pick(bytes.len()); let n = 1 + pick(32); bytes.splice(i..i, std::iter::repeat_n(0xa5, n)); }
            _ => { let (i, j) = (pick(bytes.len()), pick(bytes.len())); bytes.swap(i, j); if bytes == file { continue; } }
        }
        let out = Rc::new(RefCell::new(Out::default()));
        let r = keys.decrypt_file(&root, &object, bytes.as_slice(), Box::new(Sink(out.clone(), Vec::new())), u64::MAX, &mut |_| Ok(()));
        assert!(r.is_err() && !out.borrow().committed, "corrupted file accepted (round {round})");
        let _ = keys.open_entry(&root, &object, &bytes[..bytes.len().min(ENTRY_HEADER_LEN)]);
    }
    // Header: structural corruption only (each unlock runs Argon2id).
    for round in 0..150 {
        let mut bytes = header.clone();
        if round % 2 == 0 { let i = pick(bytes.len()); bytes[i] ^= 0x80; } else { let cut = pick(bytes.len()); bytes.truncate(cut); }
        assert!(unlock(&bytes, b"pw for fuzz", &mut ok).is_err(), "corrupted header accepted (round {round})");
    }
}
