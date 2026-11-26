//! Host-only SAF deletion boundary. Kotlin owns the pinned parent descriptor
//! and validates its grant. This function duplicates that directory through
//! procfs, then uses typed, nonrecursive unlinkat through cap-std. No path is
//! reconstructed from readlink/canonicalize and no unsafe raw-FD borrow is used.
use crate::{BridgeError, Result};

#[uniffi::export]
pub fn saf_delete_at(parent_fd: i32, name: String, directory: bool) -> Result<()> {
    if parent_fd < 0 { return Err(BridgeError::Invalid); }
    let path = paranoic_core::ResourcePath::parse(name.clone())?;
    if path.is_root() || !path.parent_and_name()?.0.is_root() { return Err(BridgeError::Invalid); }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let parent = cap_std::fs::Dir::open_ambient_dir(format!("/proc/self/fd/{parent_fd}"), cap_std::ambient_authority())
            .map_err(paranoic_core::CoreError::io)?;
        let result = if directory { parent.remove_dir(&name) } else { parent.remove_file(&name) };
        result.map_err(paranoic_core::CoreError::io)?;
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    { let _ = directory; Err(BridgeError::Unsupported) }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::{fs, os::{fd::AsRawFd, unix::fs::symlink}};

    #[test] fn saf_delete_at_is_typed_nonrecursive_and_single_component() {
        let root = std::env::temp_dir().join(format!("paranoic-delete-at-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let parent = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        fs::create_dir(root.join("folder")).unwrap();
        fs::write(root.join("folder/late-child"), b"keep").unwrap();
        assert!(saf_delete_at(parent.as_raw_fd(), "folder".into(), true).is_err());
        assert!(saf_delete_at(parent.as_raw_fd(), "folder".into(), false).is_err());
        assert_eq!(fs::read(root.join("folder/late-child")).unwrap(), b"keep");
        fs::write(root.join("file"), b"replacement").unwrap();
        assert!(saf_delete_at(parent.as_raw_fd(), "file".into(), true).is_err());
        for name in ["", "..", "../file", "folder/late-child", "/file"] {
            assert!(saf_delete_at(parent.as_raw_fd(), name.into(), false).is_err());
        }
        assert_eq!(fs::read(root.join("file")).unwrap(), b"replacement");
        saf_delete_at(parent.as_raw_fd(), "file".into(), false).unwrap();
        fs::create_dir(root.join("empty")).unwrap();
        saf_delete_at(parent.as_raw_fd(), "empty".into(), true).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test] fn saf_delete_at_uses_the_pinned_parent_after_name_substitution() {
        let root = std::env::temp_dir().join(format!("paranoic-delete-pin-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("parent")).unwrap();
        fs::create_dir(root.join("outside")).unwrap();
        fs::write(root.join("parent/file"), b"original").unwrap();
        fs::write(root.join("outside/file"), b"keep outside").unwrap();
        let parent = cap_std::fs::Dir::open_ambient_dir(root.join("parent"), cap_std::ambient_authority()).unwrap();
        fs::rename(root.join("parent"), root.join("moved")).unwrap();
        symlink(root.join("outside"), root.join("parent")).unwrap();
        saf_delete_at(parent.as_raw_fd(), "file".into(), false).unwrap();
        assert!(!root.join("moved/file").exists());
        assert_eq!(fs::read(root.join("outside/file")).unwrap(), b"keep outside");
        fs::remove_dir_all(root).unwrap();
    }
}
