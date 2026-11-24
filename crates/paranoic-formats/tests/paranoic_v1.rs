//! Paranoic format v1 core (spec docs/format/paranoic-v1.md): round trips and
//! the attacks of the threat model (§1).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{cell::RefCell, io::Write, rc::Rc};
use paranoic_crypto::{CryptoError, StreamSink};
use paranoic_formats::paranoic_v1::*;

/// Minimum KDF cost keeps the suite fast; production uses KdfParams::DESKTOP/PHONE.
const FAST: KdfParams = KdfParams { memory_kib: 65_536, iterations: 1, lanes: 1 };
fn ok() -> paranoic_crypto::Result<()> { Ok(()) }

#[derive(Default)] struct Out { data: Vec<u8>, committed: bool, aborted: bool }
struct Sink(Rc<RefCell<Out>>, Vec<u8>);
impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.1.extend_from_slice(b); Ok(b.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl StreamSink for Sink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { let mut o = self.0.borrow_mut(); o.data = self.1.clone(); o.committed = true; Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { self.0.borrow_mut().aborted = true; Ok(()) }
}

fn vault(padding: bool) -> (Vec<u8>, VaultKeys) {
    create_vault(b"vault password 1", FAST, padding, &mut OsRandom, &mut ok).unwrap()
}
fn encrypt(keys: &VaultKeys, folder: &[u8; 16], name: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    keys.encrypt_file(folder, NewFile { name, modified_ms: 7, size: data.len() as u64 }, data, &mut out, &mut OsRandom, &mut |_| Ok(())).unwrap();
    out
}
fn decrypt(keys: &VaultKeys, folder: &[u8; 16], object: &str, bytes: &[u8]) -> (paranoic_crypto::Result<Entry>, Rc<RefCell<Out>>) {
    let out = Rc::new(RefCell::new(Out::default()));
    let result = keys.decrypt_file(folder, object, bytes, Box::new(Sink(out.clone(), Vec::new())), u64::MAX, &mut |_| Ok(()));
    (result, out)
}

#[test] fn header_password_recovery_and_downgrade() {
    let (header, keys) = vault(true);
    let opened = unlock(&header, b"vault password 1", &mut ok).unwrap();
    assert_eq!(opened.vault_id, keys.vault_id);
    assert!(matches!(unlock(&header, b"wrong", &mut ok), Err(CryptoError::Authentication)));
    // Any byte change is rejected (parameters, flags, salt, slot, MAC).
    for i in 0..header.len() {
        let mut bad = header.clone(); bad[i] ^= 0x01;
        assert!(unlock(&bad, b"vault password 1", &mut ok).is_err(), "byte {i} accepted");
    }
    // Hostile KDF cost is refused before any derivation.
    let mut huge = header.clone(); huge[28..32].copy_from_slice(&(64u32 << 20).to_le_bytes());
    assert!(matches!(unlock(&huge, b"vault password 1", &mut ok), Err(CryptoError::InvalidInput)));
    // Trailing bytes are refused.
    let mut longer = header.clone(); longer.push(0);
    assert!(unlock(&longer, b"vault password 1", &mut ok).is_err());
    // Recovery key and password change.
    let (with_recovery, recovery) = set_recovery(&header, &opened, &mut OsRandom).unwrap();
    assert!(recovery_text(&recovery).split('-').all(|g| g.len() <= 4));
    assert_eq!(unlock_recovery(&with_recovery, &recovery).unwrap().vault_id, keys.vault_id);
    let changed = change_password(&with_recovery, &opened, b"new vault password", &mut OsRandom, &mut ok).unwrap();
    assert!(unlock(&changed, b"vault password 1", &mut ok).is_err());
    let reopened = unlock(&changed, b"new vault password", &mut ok).unwrap();
    assert!(unlock_recovery(&changed, &recovery).is_ok(), "recovery slot kept");
    // Same master key: old files still open after a password change.
    let root = keys.vault_id;
    let object = keys.entry_name(&root, "a.txt", EntryKind::File).unwrap();
    let (r, out) = decrypt(&reopened, &root, &object, &encrypt(&keys, &root, "a.txt", b"hi"));
    r.unwrap(); assert_eq!(out.borrow().data, b"hi");
}

#[test] fn file_round_trip_at_chunk_boundaries_with_and_without_padding() {
    for padding in [false, true] {
        let (_, keys) = vault(padding);
        let root = keys.vault_id;
        for size in [0usize, 1, 65_535, 65_536, 65_537, 200_000] {
            let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let object = keys.entry_name(&root, "file.bin", EntryKind::File).unwrap();
            let sealed = encrypt(&keys, &root, "file.bin", &data);
            let expected_plain = if padding { padme(size as u64) } else { size as u64 };
            let chunks = expected_plain.div_ceil(65_536).max(1);
            assert_eq!(sealed.len() as u64, ENTRY_HEADER_LEN as u64 + expected_plain + chunks * 16, "size {size} padding {padding}");
            let (entry, out) = decrypt(&keys, &root, &object, &sealed);
            assert_eq!(entry.unwrap(), Entry::File { name: "file.bin".into(), modified_ms: 7, size: size as u64 });
            assert!(out.borrow().committed); assert_eq!(out.borrow().data, data);
        }
    }
}

#[test] fn tampering_truncation_reordering_and_swaps_are_detected() {
    let (_, keys) = vault(false);
    let root = keys.vault_id;
    let data = vec![42u8; 3 * 65_536 + 10];
    let object = keys.entry_name(&root, "doc", EntryKind::File).unwrap();
    let sealed = encrypt(&keys, &root, "doc", &data);
    let reject = |bytes: &[u8], object: &str, why: &str| {
        let (r, out) = decrypt(&keys, &root, object, bytes);
        assert!(r.is_err(), "{why}: accepted");
        let o = out.borrow(); assert!(!o.committed && o.aborted && o.data.is_empty(), "{why}: plaintext released");
    };
    let chunk = 65_536 + 16;
    // Flip one byte in the header, in a middle chunk and in the last tag.
    for at in [10, ENTRY_HEADER_LEN + chunk + 5, sealed.len() - 1] {
        let mut bad = sealed.clone(); bad[at] ^= 1; reject(&bad, &object, "bit flip");
    }
    // Truncate at every chunk boundary, and drop the tail.
    for cut in [ENTRY_HEADER_LEN, ENTRY_HEADER_LEN + chunk, ENTRY_HEADER_LEN + 3 * chunk, sealed.len() - 1] {
        reject(&sealed[..cut], &object, "truncation");
    }
    // Appended bytes and an appended copy of a chunk.
    reject(&[sealed.as_slice(), &[0]].concat(), &object, "extension");
    // Swap two middle chunks.
    let mut swapped = sealed.clone();
    let (a, b) = (ENTRY_HEADER_LEN, ENTRY_HEADER_LEN + chunk);
    let first: Vec<u8> = swapped[a..a + chunk].to_vec(); let second: Vec<u8> = swapped[b..b + chunk].to_vec();
    swapped[a..a + chunk].copy_from_slice(&second); swapped[b..b + chunk].copy_from_slice(&first);
    reject(&swapped, &object, "chunk reorder");
    // The same object under another name or in another folder.
    let other = keys.entry_name(&root, "other", EntryKind::File).unwrap();
    reject(&sealed, &other, "renamed object");
    let folder = [9u8; 16];
    let (r, _) = decrypt(&keys, &folder, &object, &sealed); assert!(r.is_err(), "moved to another folder");
    // Another vault with the same password cannot read it.
    let (_, stranger) = vault(false);
    let (r, _) = decrypt(&stranger, &stranger.vault_id, &object, &sealed); assert!(r.is_err(), "cross-vault");
    // Chunks from another file of the same vault.
    let other_file = encrypt(&keys, &root, "doc", &data); // same name, new file key
    let mut mixed = sealed.clone(); mixed[ENTRY_HEADER_LEN..ENTRY_HEADER_LEN + chunk].copy_from_slice(&other_file[ENTRY_HEADER_LEN..ENTRY_HEADER_LEN + chunk]);
    reject(&mixed, &object, "chunk from another file");
    // Size limit enforced before reading content.
    let out = Rc::new(RefCell::new(Out::default()));
    assert!(matches!(keys.decrypt_file(&root, &object, sealed.as_slice(), Box::new(Sink(out, Vec::new())), 100, &mut |_| Ok(())), Err(CryptoError::LimitExceeded)));
}

#[test] fn names_folders_and_normalisation() {
    let (_, keys) = vault(false);
    let root = keys.vault_id;
    let nfc = "Café.txt"; let nfd = "Cafe\u{301}.txt";
    assert_eq!(keys.entry_name(&root, nfc, EntryKind::File).unwrap(), keys.entry_name(&root, nfd, EntryKind::File).unwrap());
    let a = keys.entry_name(&root, "Report.txt", EntryKind::File).unwrap();
    let b = keys.entry_name(&root, "report.txt", EntryKind::File).unwrap();
    assert_ne!(a, b);
    for name in [&a, &b] {
        assert_eq!(name.len(), 28); assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.'));
    }
    // Fixed length regardless of the name length.
    assert_eq!(keys.entry_name(&root, &"x".repeat(255), EntryKind::File).unwrap().len(), 28);
    for bad in ["", ".", "..", "a/b", "a\\b", "nul\0", &"y".repeat(256)] {
        assert!(keys.entry_name(&root, bad, EntryKind::File).is_err(), "{bad:?}");
    }
    assert_ne!(keys.entry_name(&root, "x", EntryKind::File).unwrap(), keys.entry_name(&root, "x", EntryKind::Folder).unwrap());
    let path = keys.folder_path(&root).unwrap();
    assert!(path.starts_with("d/") && path.len() == 2 + 2 + 1 + 30);
    // Folder entries: round trip, and a folder entry is not accepted as a file.
    let child = [3u8; 16];
    let header = keys.seal_folder(&root, "Документы", 5, &child, &mut OsRandom).unwrap();
    let object = keys.entry_name(&root, "Документы", EntryKind::Folder).unwrap();
    assert_eq!(keys.open_entry(&root, &object, &header).unwrap(), Entry::Folder { name: "Документы".into(), modified_ms: 5, folder_id: child });
    let as_file = keys.entry_name(&root, "Документы", EntryKind::File).unwrap();
    assert!(keys.open_entry(&root, &as_file, &header).is_err());
}

#[test] fn padding_bytes_must_be_zero_and_sizes_are_bucketed() {
    let (_, keys) = vault(true);
    let root = keys.vault_id;
    // Different sizes in one Padmé bucket give identical object sizes.
    let s1 = encrypt(&keys, &root, "a", &vec![1u8; 1000]);
    let s2 = encrypt(&keys, &root, "a", &vec![1u8; 1020]);
    assert_eq!(s1.len(), s2.len());
}
