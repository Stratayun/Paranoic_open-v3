#![no_main]
//! File object from an untrusted store: decryption either returns exactly the
//! original plaintext (and commits) or fails and aborts — never commits partial
//! or altered data.
use libfuzzer_sys::fuzz_target;
use paranoic_fuzz::{fixture, Sink};

fuzz_target!(|data: &[u8]| {
    for padding in [true, false] {
        let fx = fixture(padding);
        let sink = Sink::default();
        let result = fx.keys.decrypt_file(&fx.keys.vault_id, &fx.file.0, data, Box::new(sink.clone()), u64::MAX, &mut |_| Ok(()));
        let state = sink.0.borrow();
        match result {
            Ok(_) => {
                assert_eq!(state.1, Some(true));
                assert_eq!(state.0, fx.file_plain, "altered plaintext committed");
                assert_eq!(data, fx.file.1.as_slice(), "forged object accepted");
            }
            Err(_) => assert_ne!(state.1, Some(true), "commit after failure"),
        }
    }
});
