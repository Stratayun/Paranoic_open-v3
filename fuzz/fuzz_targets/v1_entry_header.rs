#![no_main]
//! Entry header found under some object name: only an authentic header at its
//! own name opens; anything else is rejected without panicking.
use libfuzzer_sys::fuzz_target;
use paranoic_formats::paranoic_v1::Entry;
use paranoic_fuzz::fixture;

fuzz_target!(|data: &[u8]| {
    let fx = fixture(true);
    let root = fx.keys.vault_id;
    let header = &fx.file.1[..paranoic_formats::paranoic_v1::ENTRY_HEADER_LEN];
    for (object, genuine) in [(&fx.file.0, header), (&fx.folder.0, fx.folder.1.as_slice())] {
        if let Ok(entry) = fx.keys.open_entry(&root, object, data) {
            assert_eq!(data, genuine, "forged entry accepted");
            match entry { Entry::File { name, .. } | Entry::Folder { name, .. } => assert!(!name.is_empty()) }
        }
    }
    // Swapped names: a genuine header must not open under another entry's name.
    let _ = fx.keys.open_entry(&root, &fx.folder.0, header).err().expect("file header opened as folder");
});
