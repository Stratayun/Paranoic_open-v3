//! Android shared storage (`/storage/emulated/N`, SD cards) reached with the
//! "All files access" permission is a FUSE mount served by MediaProvider. It
//! differs from an ordinary Linux filesystem in three ways that matter here:
//!
//! * `renameat2` flags are rejected (`EINVAL`), so `RENAME_NOREPLACE` fails;
//! * hard links are refused, so the `link()` fallback fails too;
//! * owner and mode bits are synthesised by the daemon (no private `0700`).
//!
//! Grants opened with [`crate::LocalStorage::open_shared`] use the helpers
//! below. Every other grant keeps the strict kernel primitives unchanged.
//!
//! Privacy of staging folders is not a guarantee shared storage can give:
//! every app holding the same permission reads the published result anyway.
//! Staging there only provides atomic publication, never confidentiality.

#[cfg(any(target_os = "linux", target_os = "android"))]
use rustix::{fd::AsFd, fs::{AtFlags, FileType, Mode, OFlags, RenameFlags}, io::Errno};

/// Create-if-absent rename. On shared storage, where the kernel flag is
/// unavailable, the target name is first reserved atomically (`O_EXCL` file
/// or `mkdir`, both fail on any existing entry, dangling symlinks included);
/// only that empty reservation is then replaced. A concurrent writer who
/// replaces the reservation in between is detected by identity and keeps
/// its entry: the call fails with `EEXIST` instead.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn rename_noreplace<A: AsFd, B: AsFd>(from: A, from_name: &str, to: B, to_name: &str,
    shared: bool) -> rustix::io::Result<()> {
    match rustix::fs::renameat_with(&from, from_name, &to, to_name, RenameFlags::NOREPLACE) {
        Err(e) if shared && (e == Errno::INVAL || e == Errno::NOSYS) => reserve_then_rename(&from, from_name, &to, to_name),
        other => other,
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn reserve_then_rename<A: AsFd, B: AsFd>(from: &A, from_name: &str, to: &B, to_name: &str) -> rustix::io::Result<()> {
    let source = rustix::fs::statat(from, from_name, AtFlags::SYMLINK_NOFOLLOW)?;
    let directory = FileType::from_raw_mode(source.st_mode) == FileType::Directory;
    if directory {
        rustix::fs::mkdirat(to, to_name, Mode::RWXU)?;
    } else {
        drop(rustix::fs::openat(to, to_name,
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR)?);
    }
    let reserved = rustix::fs::statat(to, to_name, AtFlags::SYMLINK_NOFOLLOW)?;
    let release = |to: &B| {
        // Remove only the reservation this call made, identified by inode.
        if let Ok(now) = rustix::fs::statat(to, to_name, AtFlags::SYMLINK_NOFOLLOW) {
            if now.st_dev == reserved.st_dev && now.st_ino == reserved.st_ino && (directory || now.st_size == 0) {
                let flags = if directory { AtFlags::REMOVEDIR } else { AtFlags::empty() };
                let _ = rustix::fs::unlinkat(to, to_name, flags);
            }
        }
    };
    // Re-check right before the replacing rename: still ours and still empty.
    match rustix::fs::statat(to, to_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(now) if now.st_dev == reserved.st_dev && now.st_ino == reserved.st_ino
            && (directory || now.st_size == 0) => {}
        Ok(_) => return Err(Errno::EXIST),
        Err(e) => { release(to); return Err(e); }
    }
    // Replacing an empty directory with a directory, or a file with a file, is
    // plain POSIX rename. A non-empty reservation makes it fail (ENOTEMPTY).
    rustix::fs::renameat(from, from_name, to, to_name).inspect_err(|_| release(to))
}

/// Result of [`probe_noreplace`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoReplaceProbe {
    /// The kernel/FUSE honours RENAME_NOREPLACE: atomic create-if-absent works.
    Supported,
    /// The flag is rejected (EINVAL/ENOSYS): the reservation fallback is used.
    Unsupported,
    /// The flag was IGNORED and an existing file was replaced. Dangerous.
    Ignored,
    /// The probe itself could not run (no write access, I/O error).
    Failed,
}

/// Owner's diagnostics (audit finding 1): checks, inside `root`, whether
/// `renameat2(RENAME_NOREPLACE)` really refuses to replace. Works in a fresh
/// hidden folder of its own and removes it afterwards; touches nothing else.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn probe_noreplace(root: &std::path::Path) -> NoReplaceProbe {
    use std::fs;
    let mut id = [0u8; 8];
    if getrandom_fill(&mut id).is_err() { return NoReplaceProbe::Failed; }
    let dir = root.join(format!(".paranoic-probe-{}", id.iter().map(|b| format!("{b:02x}")).collect::<String>()));
    if fs::create_dir(&dir).is_err() { return NoReplaceProbe::Failed; }
    let result = (|| -> std::io::Result<NoReplaceProbe> {
        let (a, b) = (dir.join("a"), dir.join("b"));
        fs::write(&a, b"A")?; fs::write(&b, b"B")?;
        let cwd = rustix::fs::CWD;
        let outcome = rustix::fs::renameat_with(cwd, &a, cwd, &b, RenameFlags::NOREPLACE);
        let now = fs::read(&b)?;
        Ok(match outcome {
            Err(e) if e == Errno::EXIST && now == b"B" => NoReplaceProbe::Supported,
            Err(e) if (e == Errno::INVAL || e == Errno::NOSYS) && now == b"B" => NoReplaceProbe::Unsupported,
            Ok(()) if now == b"A" => NoReplaceProbe::Ignored,
            _ => NoReplaceProbe::Failed,
        })
    })();
    let _ = fs::remove_dir_all(&dir);
    result.unwrap_or(NoReplaceProbe::Failed)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn getrandom_fill(out: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(out)
}

/// Staging folders must be private to this user, except on shared storage
/// whose ownership and mode bits are synthesised (see module docs).
#[cfg(unix)]
pub(crate) fn staging_private(metadata: &cap_std::fs::Metadata, shared: bool) -> bool {
    use cap_std::fs::{MetadataExt, PermissionsExt};
    shared || (metadata.uid() == rustix::process::geteuid().as_raw() && metadata.permissions().mode() & 0o077 == 0)
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
mod tests {
    use super::*;
    use cap_std::{ambient_authority, fs::Dir};

    struct Temp(std::path::PathBuf);
    impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn dir() -> (Temp, Dir) {
        let path = std::env::temp_dir().join(format!("paranoic-shared-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        let dir = Dir::open_ambient_dir(&path, ambient_authority()).unwrap();
        (Temp(path), dir)
    }

    #[test]
    fn fallback_moves_file_and_directory_into_absent_names() {
        let (_t, d) = dir();
        d.write("a", b"payload").unwrap();
        reserve_then_rename(&d, "a", &d, "b").unwrap();
        assert_eq!(d.read("b").unwrap(), b"payload");
        assert!(!d.exists("a"));
        d.create_dir("src").unwrap();
        d.write("src/x", b"1").unwrap();
        reserve_then_rename(&d, "src", &d, "dst").unwrap();
        assert_eq!(d.read("dst/x").unwrap(), b"1");
    }

    #[test]
    fn fallback_never_replaces_existing_entries() {
        let (_t, d) = dir();
        d.write("a", b"new").unwrap();
        d.write("b", b"keep").unwrap();
        assert_eq!(reserve_then_rename(&d, "a", &d, "b"), Err(Errno::EXIST));
        assert_eq!(d.read("b").unwrap(), b"keep");
        assert_eq!(d.read("a").unwrap(), b"new");
        d.symlink("missing", "dangling").unwrap();
        assert_eq!(reserve_then_rename(&d, "a", &d, "dangling"), Err(Errno::EXIST));
        d.create_dir("full").unwrap();
        d.create_dir("empty").unwrap();
        assert_eq!(reserve_then_rename(&d, "full", &d, "empty"), Err(Errno::EXIST));
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
mod probe_tests {
    #[test] fn probe_reports_supported_on_a_normal_linux_filesystem_and_cleans_up() {
        let root = std::env::temp_dir().join(format!("paranoic-probe-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(super::probe_noreplace(&root), super::NoReplaceProbe::Supported);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "probe folder left behind");
        std::fs::remove_dir(&root).unwrap();
    }
}
