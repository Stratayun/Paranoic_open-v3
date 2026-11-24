//! QuickXorHash (OneDrive) against reference vectors from rclone
//! (backend/onedrive/quickxorhash/quickxorhash_test.go, MIT licence), an
//! independent implementation used against the live OneDrive API.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use base64::{engine::general_purpose::STANDARD, Engine};
use paranoic_cloud::QuickXorHash;

const VECTORS: &[(usize, &str, &str)] = &[
    (0, "", "AAAAAAAAAAAAAAAAAAAAAAAAAAA="),
    (1, "Sg==", "SgAAAAAAAAAAAAAAAQAAAAAAAAA="),
    (2, "tbQ=", "taAFAAAAAAAAAAAAAgAAAAAAAAA="),
    (3, "0pZP", "0rDEEwAAAAAAAAAAAwAAAAAAAAA="),
    (8, "/aNzzCFPS/A=", "/RjFHJgRgicsAR4ACAAAAAAAAAA="),
    (13, "vPgoDjOfO6fm71RxLw==", "vMAHChwwg0/s4BTmdQcV4vACAAA="),
    (15, "gQaybEqS/4UlDc8e4IJm", "gDCALNigBEn8oxAlZ8AzPAAOQZg="),
    (16, "2fuxhBJXtpWFe8dOfdGeHw==", "O9tHLAghgSvYohKFyMMxnNCHaHg="),
    (17, "XBV6YKU9V7yMakZnFIxIkuU=", "HbplHsBQih5cgReMQYMRzkABRiA="),
    (20, "T6LYJIfDh81JrAK309H2JMJTXis=", "zBTHrspn3mEcohlJdIUAbjGNaNg="),
    (21, "DWAAX5/CIfrmErgZa8ot6ZraeSbu", "LR2Z0PjuRYGKQB/mhQAuMrAGZbQ="),
    (32, "NbYXsp5/K6mR+NmHwExjvWeWDJFnXTKWVlzYHoesp2E=", "wjuAuWDiq04qDt1R8hHWDDcwVoQ="),
    (64, "Mb7EGva2rEE5fENDL85P+BsapHEEjv2/siVhKjvAQe02feExVOQSkfmuYzU/kTF1MaKjPmKF/w+cbvwfdWL8aQ==", "n1anP5NfvD4XDYWIeRPW3ZkPv1Y="),
    (128, "ikwCorI7PKWz17EI50jZCGbV9JU2E8bXVfxNMg5zdmqSZ2NlsQPp0kqYIPjzwTg1MBtfWPg53k0h0P2naJNEVgrqpoHTfV2b3pJ4m0zYPTJmUX4Bg/lOxcnCxAYKU29Y5F0U8Quz7ZXFBEweftXxJ7RS4r6N7BzJrPsLhY7hgck=", "imAoFvCWlDn4yVw3/oq1PDbbm6U="),
    (333, "w0LGhqU1WXFbdavqDE4kAjEzWLGGzmTNikzqnsiXHx2KRReKVTxkv27u3UcEz9+lbMvYl4xFf2Z4aE1xRBBNd1Ke5C0zToSaYw5o4B/7X99nKK2/XaUX1byLow2aju2XJl2OpKpJg+tSJ2fmjIJTkfuYUz574dFX6/VXxSxwGH/xQEAKS5TCsBK3CwnuG1p5SAsQq3gGVozDWyjEBcWDMdy8/AIFrj/y03Lfc/RNRCQTAfZbnf2QwV7sluw4fH3XJr07UoD0YqN+7XZzidtrwqMY26fpLZnyZjnBEt1FAZWO7RnKG5asg8xRk9YaDdedXdQSJAOy6bWEWlABj+tVAigBxavaluUH8LOj+yfCFldJjNLdi90fVHkUD/m4Mr5OtmupNMXPwuG3EQlqWUVpQoYpUYKLsk7a5Mvg6UFkiH596y5IbJEVCI1Kb3D1", "e3+wo77iKcILiZegnzyUNcjCdoQ="),
];

#[test] fn matches_reference_vectors_in_one_call_and_in_blocks() {
    for (size, input, expected) in VECTORS {
        let data = STANDARD.decode(input).unwrap();
        assert_eq!(data.len(), *size);
        let mut whole = QuickXorHash::new(); whole.update(&data);
        assert_eq!(whole.finalize_base64(), *expected, "size {size}");
        for block in [1usize, 2, 7, 8, 64, 100] {
            let mut h = QuickXorHash::new();
            for part in data.chunks(block) { h.update(part); }
            assert_eq!(h.finalize_base64(), *expected, "size {size} block {block}");
        }
    }
}
