//! Plan 1.4 part 1: age through NativeRuntime (AES Crypt removed).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::*;
use std::{fs, path::PathBuf};

use crate::test_support::{NoDocs, NoSealer};
struct Fixture(PathBuf);
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[test] fn age_files() {
    let fx = Fixture(std::env::temp_dir().join(format!("paranoic-cryptofiles-{}", Uuid::new_v4())));
    for d in ["state", "workspace/out"] { fs::create_dir_all(fx.0.join(d)).unwrap(); }
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&fx.0, fs::Permissions::from_mode(0o700)).unwrap(); }
    let rt = NativeRuntime::new(fx.0.join("state").to_str().unwrap().into(), fx.0.join("workspace").to_str().unwrap().into(),
        String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
    let app = rt.spaces().unwrap()[0].root.clone();
    let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };
    let ws = |p: &str| fx.0.join("workspace").join(p);
    let secret: Vec<u8> = (0..200_000u32).flat_map(|i| i.to_le_bytes()).chain(b"top secret marker".iter().copied()).collect();
    fs::write(ws("doc.bin"), &secret).unwrap();
    let password = "correct horse battery staple";
    // age is the only single-file format (P-10); the block keeps its scope.
    {
        let (format, ext) = (CRYPTO_FORMAT_AGE, "age");
        let enc = format!("doc.bin.{ext}");
        rt.encrypt_file(at("doc.bin"), at(""), enc.clone(), format, password.into()).unwrap();
        let ciphertext = fs::read(ws(&enc)).unwrap();
        assert!(!ciphertext.windows(17).any(|w| w == b"top secret marker"), "{ext}: plaintext visible");
        assert_eq!(fs::read(ws("doc.bin")).unwrap(), secret, "{ext}: source changed");
        // Never replaces an existing file.
        assert!(matches!(rt.encrypt_file(at("doc.bin"), at(""), enc.clone(), format, password.into()), Err(BridgeError::Conflict)));
        assert_eq!(fs::read(ws(&enc)).unwrap(), ciphertext);
        // Wrong password: refused, nothing written.
        assert!(matches!(rt.decrypt_file(at(&enc), at("out"), format!("wrong.{ext}"), format, "not the password".into()),
            Err(BridgeError::WrongPassword)), "{ext}");
        assert!(!ws(&format!("out/wrong.{ext}")).exists());
        // Correct password: exact plaintext.
        rt.decrypt_file(at(&enc), at("out"), format!("plain.{ext}"), format, password.into()).unwrap();
        assert_eq!(fs::read(ws(&format!("out/plain.{ext}"))).unwrap(), secret, "{ext}");
        // Tampered byte: refused, nothing written.
        let mut bad = ciphertext.clone(); let i = bad.len() / 2; bad[i] ^= 1;
        fs::write(ws(&format!("bad.{ext}")), &bad).unwrap();
        assert!(rt.decrypt_file(at(&format!("bad.{ext}")), at("out"), format!("bad.{ext}"), format, password.into()).is_err(), "{ext}: tamper");
        assert!(!ws(&format!("out/bad.{ext}")).exists(), "{ext}: tampered output published");
        // The removed AES Crypt code (2) is refused, not mistaken for age.
        assert!(matches!(rt.decrypt_file(at(&enc), at("out"), format!("x.{ext}"), 2, password.into()), Err(BridgeError::Invalid)));
    }
    assert!(matches!(rt.encrypt_file(at("doc.bin"), at(""), "z".into(), 9, password.into()), Err(BridgeError::Invalid)));
    assert!(!fs::read_dir(ws("")).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".pnc-spool")), "spool left");
    rt.shutdown().unwrap();
}
