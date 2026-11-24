#![no_main]
//! Vault header from an untrusted store: parsing must never panic, and only the
//! genuine header may unlock (header MAC + AEAD slot).
use libfuzzer_sys::fuzz_target;
use paranoic_formats::paranoic_v1 as v1;
use paranoic_fuzz::fixture;

fuzz_target!(|data: &[u8]| {
    let fx = fixture(true);
    if let Ok(keys) = v1::unlock_recovery(data, &fx.recovery) {
        assert_eq!(data, fx.header.as_slice(), "forged header accepted");
        assert_eq!(keys.vault_id, fx.keys.vault_id);
    }
    // Password path up to the KDF: wrong-length / malformed input must fail
    // before Argon2 runs on attacker parameters (bounded by validate()).
    if data.len() != fx.header.len() {
        assert!(v1::unlock(data, b"fuzz password for the fixture", &mut || Ok(())).is_err());
    }
});
