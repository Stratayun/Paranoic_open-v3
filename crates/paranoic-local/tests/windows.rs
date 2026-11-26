//! Windows local storage: writes into user folders through owner-only staging,
//! no-replace publication and moves, FileId binding (registers У-3, У-19).
#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::io::Write;
use paranoic_core::{JobId, ResourcePath, Storage, StorageId};
use paranoic_local::{Access, LocalStorage};

fn temp() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("paranoic-local-win-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&p).unwrap(); p
}
fn rp(p: &str) -> ResourcePath { ResourcePath::parse(p).unwrap() }

#[test] fn user_folder_is_writable_without_replacing_anything() {
    let root = temp();
    let storage = LocalStorage::open(StorageId(uuid::Uuid::new_v4()), &root, Access::ReadWrite).unwrap();
    let caps = storage.capabilities();
    assert!(caps.write && caps.delete && caps.atomic_move_no_replace, "{caps:?}");
    let mut staged = storage.stage(&rp("a.txt"), JobId(uuid::Uuid::new_v4())).unwrap();
    staged.write_all(b"first").unwrap();
    staged.publish().unwrap();
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first");
    // Staging leaves nothing behind.
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    // Existing target: refused, untouched.
    let mut staged = storage.stage(&rp("a.txt"), JobId(uuid::Uuid::new_v4())).unwrap();
    staged.write_all(b"second").unwrap();
    assert!(staged.publish().is_err());
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first");
    // Move without replacement.
    std::fs::write(root.join("b.txt"), b"other").unwrap();
    assert!(storage.move_no_replace(&rp("b.txt"), &rp("a.txt")).is_err());
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"first");
    storage.move_no_replace(&rp("b.txt"), &rp("c.txt")).unwrap();
    assert!(!root.join("b.txt").exists() && std::fs::read(root.join("c.txt")).unwrap() == b"other");
    storage.create_directory(&rp("sub")).unwrap();
    storage.move_no_replace(&rp("c.txt"), &rp("sub/c.txt")).unwrap();
    storage.delete_file(&rp("sub/c.txt")).unwrap();
    storage.delete_empty_directory(&rp("sub")).unwrap();
    storage.create_directory(&rp("folder")).unwrap();
    std::fs::write(root.join("folder/keep.txt"), b"directory contents").unwrap();
    storage.move_no_replace(&rp("folder"), &rp("перенесённая папка")).unwrap();
    assert_eq!(std::fs::read(root.join("перенесённая папка/keep.txt")).unwrap(), b"directory contents");
    drop(storage); // the open root handle blocks deleting the folder on Windows
    std::fs::remove_dir_all(&root).unwrap();
}

#[test] fn binding_uses_the_folder_identity() {
    let (a, b) = (temp(), temp());
    let sa = LocalStorage::open(StorageId(uuid::Uuid::new_v4()), &a, Access::ReadOnly).unwrap();
    let sa2 = LocalStorage::open(StorageId(uuid::Uuid::new_v4()), &a, Access::ReadOnly).unwrap();
    let sb = LocalStorage::open(StorageId(uuid::Uuid::new_v4()), &b, Access::ReadOnly).unwrap();
    let (ka, ka2, kb) = (sa.binding_key().unwrap(), sa2.binding_key().unwrap(), sb.binding_key().unwrap());
    assert!(ka.starts_with("native-windows-fileid:"), "{ka}");
    assert_eq!(ka, ka2); assert_ne!(ka, kb);
    drop((sa, sa2, sb));
    // Same name, new folder: a different identity.
    std::fs::remove_dir_all(&a).unwrap(); std::fs::create_dir(&a).unwrap();
    let again = LocalStorage::open(StorageId(uuid::Uuid::new_v4()), &a, Access::ReadOnly).unwrap();
    assert_ne!(again.binding_key().unwrap(), ka, "recreated folder kept the old binding");
    drop(again);
    std::fs::remove_dir_all(&a).unwrap(); std::fs::remove_dir_all(&b).unwrap();
}
