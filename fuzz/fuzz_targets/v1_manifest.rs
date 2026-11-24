#![no_main]
//! Version manifest (spec §9) as found in the storage: only a manifest sealed
//! with this vault's key opens; any other input is rejected without panicking.
//! A manifest that opens is re-sealed and must open to the same content.
use libfuzzer_sys::fuzz_target;
use paranoic_formats::paranoic_manifest::{open, seal, Verdict};
use paranoic_fuzz::{fixture, TestRandom};

fuzz_target!(|data: &[u8]| {
    let fx = fixture(true);
    if let Ok(manifest) = open(&fx.keys, data) {
        let again = seal(&fx.keys, &manifest, &mut TestRandom(1)).expect("an opened manifest must re-seal");
        assert_eq!(open(&fx.keys, &again).expect("re-sealed manifest must open"), manifest);
        for (locator, record) in manifest.records() {
            assert_eq!(manifest.check(locator, &record.current), Verdict::Accepted);
        }
    }
});
