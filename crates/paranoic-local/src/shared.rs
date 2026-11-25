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
/// `renameat2(RENAME_NOREPLACE)` really refuses to replace.
///
/// Safe on a shared, concurrently written volume (audit finding 23): every
/// step is relative to held directory descriptors, objects are created
/// exclusively, at most two bytes are read, and cleanup removes only the
/// entries this call created (matched by inode). Anything unexpected ends
/// the probe as [`NoReplaceProbe::Failed`] and leaves foreign entries alone.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn probe_noreplace(root: &std::path::Path) -> NoReplaceProbe {
    use rustix::fs::{fstat, mkdirat, openat, statat, unlinkat};
    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    // The granted root itself may be reached through a link (e.g. /sdcard); everything below it may not.
    let Ok(parent) = openat(rustix::fs::CWD, root, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()) else { return NoReplaceProbe::Failed };
    let mut id = [0u8; 8];
    if getrandom_fill(&mut id).is_err() { return NoReplaceProbe::Failed; }
    let name = format!(".paranoic-probe-{}", id.iter().map(|b| format!("{b:02x}")).collect::<String>());
    if mkdirat(&parent, name.as_str(), Mode::RWXU).is_err() { return NoReplaceProbe::Failed; }
    let Ok(made) = statat(&parent, name.as_str(), AtFlags::SYMLINK_NOFOLLOW) else { return NoReplaceProbe::Failed };
    let dir = match openat(&parent, name.as_str(), dir_flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(_) => return NoReplaceProbe::Failed,
    };
    // The opened directory must be the one this call created.
    match fstat(&dir) {
        Ok(st) if st.st_dev == made.st_dev && st.st_ino == made.st_ino => {}
        _ => return NoReplaceProbe::Failed,
    }
    let create = |name: &str, byte: u8| -> rustix::io::Result<rustix::fs::Stat> {
        let fd = openat(&dir, name, OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR)?;
        if rustix::io::write(&fd, &[byte])? != 1 { return Err(Errno::IO); }
        fstat(&fd)
    };
    let (a, b) = match (create("a", b'A'), create("b", b'B')) {
        (Ok(a), Ok(b)) => (a, b),
        (a, b) => {
            for (n, st) in [("a", a), ("b", b)] { if let Ok(st) = st { remove_own(&dir, n, &st); } }
            let _ = unlinkat(&parent, name.as_str(), AtFlags::REMOVEDIR);
            return NoReplaceProbe::Failed;
        }
    };
    let outcome = rustix::fs::renameat_with(&dir, "a", &dir, "b", RenameFlags::NOREPLACE);
    // Read at most two bytes of "b" without following links.
    let content = openat(&dir, "b", OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()).ok().and_then(|fd| {
        let mut buf = [0u8; 2];
        rustix::io::read(&fd, &mut buf).ok().map(|n| buf[..n].to_vec())
    });
    let verdict = match (outcome, content.as_deref()) {
        (Err(e), Some(b"B")) if e == Errno::EXIST => NoReplaceProbe::Supported,
        (Err(e), Some(b"B")) if e == Errno::INVAL || e == Errno::NOSYS => NoReplaceProbe::Unsupported,
        (Ok(()), Some(b"A")) => NoReplaceProbe::Ignored,
        _ => NoReplaceProbe::Failed,
    };
    // After an ignored flag "b" holds a's inode; remove whichever of ours remains.
    remove_own(&dir, "a", &a);
    remove_own(&dir, "b", &b);
    remove_own(&dir, "b", &a);
    // Fails (and leaves it) if anything foreign appeared inside.
    let _ = unlinkat(&parent, name.as_str(), AtFlags::REMOVEDIR);
    verdict
}

/// Unlinks `name` only if it is still the object identified by `st`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn remove_own<D: AsFd>(dir: D, name: &str, st: &rustix::fs::Stat) {
    if let Ok(now) = rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        if now.st_dev == st.st_dev && now.st_ino == st.st_ino {
            let _ = rustix::fs::unlinkat(&dir, name, AtFlags::empty());
        }
    }
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
