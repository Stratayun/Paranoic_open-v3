//! Shared fixtures for the format 1.8 (`paranoic_v1`) fuzz targets: one vault
//! created once with deterministic randomness, its recovery key, and sample
//! objects used as seeds and as the only inputs allowed to authenticate.
use paranoic_crypto::StreamSink;
use paranoic_formats::paranoic_v1::{self as v1, KdfParams, NewFile, RandomSource, VaultKeys};
use std::{cell::RefCell, io::Write, rc::Rc, sync::OnceLock};

/// Deterministic randomness (SplitMix64) — fixtures only, never production.
pub struct TestRandom(pub u64);
impl RandomSource for TestRandom {
    fn fill(&mut self, out: &mut [u8]) -> paranoic_crypto::Result<()> {
        for b in out.iter_mut() {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            *b = (z ^ (z >> 31)) as u8;
        }
        Ok(())
    }
}

pub struct Fixture {
    pub header: Vec<u8>,
    pub recovery: [u8; 32],
    pub keys: VaultKeys,
    /// (object name, bytes) of a file "sample.txt" in the root folder.
    pub file: (String, Vec<u8>),
    pub file_plain: Vec<u8>,
    /// (object name, header) of a folder "Folder" in the root folder.
    pub folder: (String, Vec<u8>),
}

const KDF: KdfParams = KdfParams { memory_kib: 65_536, iterations: 1, lanes: 1 };

pub fn fixture(padding: bool) -> &'static Fixture {
    static PADDED: OnceLock<Fixture> = OnceLock::new();
    static PLAIN: OnceLock<Fixture> = OnceLock::new();
    let cell = if padding { &PADDED } else { &PLAIN };
    cell.get_or_init(|| build(padding))
}

#[allow(clippy::unwrap_used)]
fn build(padding: bool) -> Fixture {
    let mut rng = TestRandom(if padding { 1 } else { 2 });
    let (header, keys) = v1::create_vault(b"fuzz password for the fixture", KDF, padding, &mut rng, &mut || Ok(())).unwrap();
    let (header, recovery) = v1::set_recovery(&header, &keys, &mut rng).unwrap();
    let keys = v1::unlock_recovery(&header, &recovery).unwrap();
    let root = keys.vault_id;
    let file_plain: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
    let mut bytes = Vec::new();
    keys.encrypt_file(&root, NewFile { name: "sample.txt", modified_ms: 7, size: file_plain.len() as u64 },
        &file_plain[..], &mut bytes, &mut rng, &mut |_| Ok(())).unwrap();
    let object = keys.entry_name(&root, "sample.txt", v1::EntryKind::File).unwrap();
    let child = [9u8; 16];
    let folder = keys.seal_folder(&root, "Folder", 8, &child, &mut rng).unwrap();
    let folder_object = keys.entry_name(&root, "Folder", v1::EntryKind::Folder).unwrap();
    Fixture { header, recovery: *recovery, keys, file: (object, bytes), file_plain, folder: (folder_object, folder) }
}

/// Sink recording whether the decryptor committed and what it wrote.
#[derive(Clone, Default)]
pub struct Sink(pub Rc<RefCell<(Vec<u8>, Option<bool>)>>);
impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.0.borrow_mut().0.extend_from_slice(b); Ok(b.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl StreamSink for Sink {
    fn commit(self: Box<Self>) -> paranoic_crypto::Result<()> { self.0.borrow_mut().1 = Some(true); Ok(()) }
    fn abort(self: Box<Self>) -> paranoic_crypto::Result<()> { self.0.borrow_mut().1 = Some(false); Ok(()) }
}
