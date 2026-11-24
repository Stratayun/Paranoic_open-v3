#![no_main]
//! Structure-aware: arbitrary names, sizes (around chunk and padding edges) and
//! content survive encrypt → decrypt; rename keeps the chunks; truncation,
//! extension or one flipped byte anywhere is always detected.
use libfuzzer_sys::fuzz_target;
use paranoic_formats::paranoic_v1::{EntryKind, NewFile, NewPlace};
use paranoic_fuzz::{fixture, Sink, TestRandom};

const CHUNK: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() < 12 { return; }
    let padding = data[0] & 1 == 1;
    let fx = fixture(padding);
    let keys = &fx.keys;
    let root = keys.vault_id;
    // Size near a chunk boundary or small, driven by input.
    let base = [0usize, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK][usize::from(data[1]) % 5];
    let size = base + usize::from(data[2]) % 300;
    let seed = u64::from_le_bytes([data[3], data[4], data[5], data[6], data[7], data[8], data[9], data[10]]);
    let name_len = usize::from(data[11]) % 40;
    let name: String = data.get(12..12 + name_len).unwrap_or_default().iter()
        .map(|b| char::from(b'a' + b % 26)).collect();
    let name = if name.is_empty() { "x".to_owned() } else { name };
    let plain: Vec<u8> = (0..size).map(|i| data[(i % data.len())] ^ (i as u8)).collect();

    let mut rng = TestRandom(seed);
    let mut object_bytes = Vec::new();
    keys.encrypt_file(&root, NewFile { name: &name, modified_ms: seed, size: size as u64 }, &plain[..], &mut object_bytes,
        &mut rng, &mut |_| Ok(())).expect("encrypt");
    let object = keys.entry_name(&root, &name, EntryKind::File).expect("name");
    let decrypt = |bytes: &[u8], object: &str| {
        let sink = Sink::default();
        let r = keys.decrypt_file(&root, object, bytes, Box::new(sink.clone()), u64::MAX, &mut |_| Ok(()));
        let s = sink.0.borrow().clone();
        (r.is_ok(), s)
    };
    let (ok, (out, committed)) = decrypt(&object_bytes, &object);
    assert!(ok && committed == Some(true) && out == plain, "round trip failed");

    // Damage chosen by the input: truncate, extend, or flip one byte.
    let at = (seed as usize) % object_bytes.len();
    let mut damaged = object_bytes.clone();
    match data[1] >> 4 & 3 {
        0 => damaged.truncate(at),
        1 => damaged.push(data[2]),
        _ => damaged[at] ^= 1 | data[2],
    }
    let (ok, (_, committed)) = decrypt(&damaged, &object);
    assert!(!ok && committed != Some(true), "damage not detected");

    // Rename: new header + verbatim chunks decrypt under the new name only.
    let new_name = format!("{name}-renamed");
    let header = &object_bytes[..paranoic_formats::paranoic_v1::ENTRY_HEADER_LEN];
    let reissued = keys.reissue_header(&root, &object, header, &NewPlace { folder: &root, name: &new_name, modified_ms: 1 }, &mut rng)
        .expect("reissue");
    let mut renamed = reissued;
    renamed.extend_from_slice(&object_bytes[paranoic_formats::paranoic_v1::ENTRY_HEADER_LEN..]);
    let new_object = keys.entry_name(&root, &new_name, EntryKind::File).expect("name");
    let (ok, (out, _)) = decrypt(&renamed, &new_object);
    assert!(ok && out == plain, "renamed object does not decrypt");
    assert!(!decrypt(&renamed, &object).0, "renamed header opened under the old name");
});
