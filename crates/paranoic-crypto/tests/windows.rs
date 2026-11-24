//! Windows private directory: refused unless owner-only (register У-1).
#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::io::{Read, Write};
use paranoic_crypto::{CryptoError, ObjectId, PrivateDirectory};

#[test] fn private_directory_requires_owner_only_acl() {
    let dir = std::env::temp_dir().join(format!("paranoic-crypto-win-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    // Add an "Everyone: read" grant so the folder is clearly not private.
    let status = std::process::Command::new("icacls").arg(&dir).args(["/grant", "*S-1-1-0:(R)"]).output().unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stdout));
    assert!(matches!(PrivateDirectory::open(&dir), Err(CryptoError::Permissions)), "shared folder accepted");
    paranoic_windows::ensure_owner_only(&dir).unwrap();
    let private = PrivateDirectory::open(&dir).unwrap();
    let id = ObjectId::generate().unwrap();
    let mut sink = private.stage(id).unwrap(); sink.write_all(b"secret").unwrap(); sink.commit().unwrap();
    let mut data = Vec::new(); private.open_object(id).unwrap().read_to_end(&mut data).unwrap();
    assert_eq!(data, b"secret");
    drop(private);
    std::fs::remove_dir_all(&dir).unwrap();
}
