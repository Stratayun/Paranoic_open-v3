//! Retained recovery staging for mapped wrapper renames and moves.
use super::*;
use cap_std::fs::{DirBuilderExt, OpenOptionsExt, MetadataExt, PermissionsExt};

fn open_dir(parent: &Dir, name: &str) -> Result<Dir> {
    let fd = rustix::fs::openat(parent, name, rustix::fs::OFlags::RDONLY |
        rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty()).map_err(|e| CoreError::io(e.into()))?;
    Ok(Dir::from_std_file(std::fs::File::from(fd)))
}

fn mapping(directory: &Dir, expected: &[u8]) -> Result<()> {
    let fd = rustix::fs::openat(directory, "name.c9s", rustix::fs::OFlags::RDONLY |
        rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty()).map_err(|e| CoreError::io(e.into()))?;
    let file = std::fs::File::from(fd);
    let before = file.metadata().map_err(CoreError::io)?;
    if !before.is_file() || before.len() != expected.len() as u64 {
        return Err(CoreError::new(ErrorCode::Integrity, "mapping size or kind changed"));
    }
    let mut bytes = Vec::new();
    file.take(369).read_to_end(&mut bytes).map_err(CoreError::io)?;
    if bytes != expected { return Err(CoreError::new(ErrorCode::Integrity, "mapping changed")); }
    Ok(())
}

pub(super) fn rename(storage: &LocalStorage, source: &ResourcePath, target: &ResourcePath,
    job: JobId, old: &[u8], new: &[u8]) -> Result<()> {
    storage.writable()?;
    if source == target || target.as_str().starts_with(&format!("{}/", source.as_str()))
        || old.is_empty() || new.is_empty() || old.len() > 368 || new.len() > 368 {
        return Err(CoreError::new(ErrorCode::InvalidInput, "invalid mapped rename"));
    }
    let (parent, source_name) = storage.parent(source)?;
    let (target_parent, target_name) = storage.parent(target)?;
    match target_parent.symlink_metadata(target_name) {
        Ok(_) => return Err(CoreError::new(ErrorCode::AlreadyExists, "rename target exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(CoreError::io(e)),
    }
    mapping(&open_dir(&parent, source_name)?, old)?;
    let temporary = format!(".paranoic-{}", job.0);
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    parent.create_dir_with(&temporary, &builder).map_err(CoreError::io)?;
    let staging = open_dir(&parent, &temporary)?;
    let metadata = staging.dir_metadata().map_err(CoreError::io)?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(CoreError::new(ErrorCode::PermissionDenied, "rename staging is not private"));
    }
    sync_directory(&parent)?;
    // No Drop cleanup: after detachment this is the sole reachable payload.
    // Even failures before detachment leave evidence for explicit recovery.
    rustix::fs::renameat_with(&parent, source_name, &staging, "entry", rustix::fs::RenameFlags::NOREPLACE)
        .map_err(|e| CoreError::io(e.into()))?;
    sync_directory(&staging)?;
    sync_directory(&parent)?;
    let entry = open_dir(&staging, "entry")?;
    mapping(&entry, old)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut replacement = staging.open_with("mapping", &options).map_err(CoreError::io)?;
    replacement.write_all(new).map_err(CoreError::io)?;
    replacement.sync_all().map_err(CoreError::io)?;
    drop(replacement);
    staging.rename("mapping", &entry, "name.c9s").map_err(CoreError::io)?;
    sync_directory(&entry)?;
    sync_directory(&staging)?;
    mapping(&entry, new)?;
    rustix::fs::renameat_with(&staging, "entry", &target_parent, target_name, rustix::fs::RenameFlags::NOREPLACE)
        .map_err(|e| CoreError::io(e.into()))?;
    sync_directory(&target_parent)?;
    sync_directory(&staging)?;
    // Only remove the reserved empty staging directory, never the payload.
    let visible = parent.symlink_metadata(&temporary).map_err(CoreError::io)?;
    if visible.dev() != metadata.dev() || visible.ino() != metadata.ino() {
        return Err(CoreError::new(ErrorCode::NeedsAttention, "rename staging identity changed"));
    }
    parent.remove_dir(&temporary).map_err(CoreError::io)?;
    sync_directory(&parent)
}

fn observe(parent: &Dir, name: &str, old: &[u8], new: &[u8], check: &mut dyn FnMut() -> Result<()>)
    -> Result<paranoic_core::MappedEntryObservation> {
    use paranoic_core::MappedEntryObservation as O;
    check()?;
    let before = match parent.symlink_metadata(name) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(O::Missing),
        Err(e) => return Err(CoreError::io(e)),
    };
    if !before.is_dir() { return Ok(O::Different); }
    let directory = open_dir(parent, name)?;
    let fd = match rustix::fs::openat(&directory, "name.c9s", rustix::fs::OFlags::RDONLY |
        rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty()) {
        Ok(fd) => fd,
        Err(e) if e == rustix::io::Errno::NOENT => return Ok(O::Different),
        Err(e) => return Err(CoreError::io(e.into())),
    };
    let mut file = std::fs::File::from(fd);
    let metadata = file.metadata().map_err(CoreError::io)?;
    if !metadata.is_file() || metadata.len() > 368 { return Ok(O::Different); }
    let mut bytes = Vec::new();
    (&mut file).take(369).read_to_end(&mut bytes).map_err(CoreError::io)?;
    check()?;
    let after = parent.symlink_metadata(name).map_err(CoreError::io)?;
    let held = directory.dir_metadata().map_err(CoreError::io)?;
    if before.dev() != held.dev() || before.ino() != held.ino() || after.dev() != held.dev() || after.ino() != held.ino()
        || file.metadata().map_err(CoreError::io)?.modified().ok() != metadata.modified().ok()
        || bytes.len() as u64 != metadata.len() {
        return Err(CoreError::new(ErrorCode::SourceChanged, "rename entry changed during inspection"));
    }
    Ok(match (bytes == old, bytes == new) {
        (true, true) => O::BothMappings, (true, false) => O::OldMapping,
        (false, true) => O::NewMapping, _ => O::Different,
    })
}

pub(super) fn inspect(storage: &LocalStorage, source: &ResourcePath, target: &ResourcePath,
    job: JobId, old: &[u8], new: &[u8], check: &mut dyn FnMut() -> Result<()>) -> Result<paranoic_core::MappedRenameObservation> {
    use paranoic_core::{MappedEntryObservation as O, MappedRenameObservation};
    if old.is_empty() || new.is_empty() || old.len() > 368 || new.len() > 368 {
        return Err(CoreError::new(ErrorCode::InvalidInput, "invalid rename inspection"));
    }
    check()?;
    let (parent, name) = storage.parent(source)?;
    let (destination, target_name) = storage.parent(target)?;
    let source = observe(&parent, name, old, new, check)?;
    let target = observe(&destination, target_name, old, new, check)?;
    let temporary = format!(".paranoic-{}", job.0);
    let staging = match open_dir(&parent, &temporary) {
        Ok(dir) => Some(dir),
        Err(e) if e.code == ErrorCode::NotFound => None,
        Err(e) => return Err(e),
    };
    let staged_entry = if let Some(dir) = &staging {
        let metadata = dir.dir_metadata().map_err(CoreError::io)?;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(CoreError::new(ErrorCode::PermissionDenied, "rename staging is not private"));
        }
        observe(dir, "entry", old, new, check)?
    } else { O::Missing };
    check()?;
    Ok(MappedRenameObservation { source, target, staged_entry, staging_present: staging.is_some() })
}

/// Publish the retained entry only after an explicit journaled host decision.
/// No cleanup guard owns this entry: failures must preserve the only payload.
#[allow(clippy::too_many_arguments)]
pub(super) fn recover(storage: &LocalStorage, source: &ResourcePath, target: &ResourcePath,
    original: JobId, recovery: JobId, old: &[u8], new: &[u8], action: paranoic_core::MappedRecoveryAction) -> Result<()> {
    use paranoic_core::{MappedEntryObservation as O, MappedRecoveryAction as A};
    storage.writable()?;
    if original == recovery || source == target || old.is_empty() || new.is_empty()
        || old.len() > 368 || new.len() > 368 {
        return Err(CoreError::new(ErrorCode::InvalidInput, "invalid mapped recovery request"));
    }
    let (parent, source_name) = storage.parent(source)?;
    let (target_parent, target_name) = storage.parent(target)?;
    // Ambiguous public copies require separate review, never a replacement.
    for (directory, name) in [(&parent, source_name), (&target_parent, target_name)] {
        match directory.symlink_metadata(name) {
            Ok(_) => return Err(CoreError::new(ErrorCode::AlreadyExists, "public rename entry exists; recovery refused")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(CoreError::io(e)),
        }
    }
    let temporary = format!(".paranoic-{}", original.0);
    let staging = open_dir(&parent, &temporary)?;
    let metadata = staging.dir_metadata().map_err(CoreError::io)?;
    let visible = parent.symlink_metadata(&temporary).map_err(CoreError::io)?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.permissions().mode() & 0o077 != 0
        || visible.dev() != metadata.dev() || visible.ino() != metadata.ino() {
        return Err(CoreError::new(ErrorCode::PermissionDenied, "recovery staging ownership changed"));
    }
    let state = observe(&staging, "entry", old, new, &mut || Ok(()))?;
    if !matches!(state, O::OldMapping | O::NewMapping | O::BothMappings) {
        return Err(CoreError::new(ErrorCode::Integrity, "staged entry has no recognized mapping"));
    }
    let entry = open_dir(&staging, "entry")?;
    let expected = match action { A::Complete => new, A::RestoreSource => old };
    // Use this recovery job's unique name. Never truncate a prior attempt's file.
    let replacement_name = format!("mapping-{}", recovery.0);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut replacement = staging.open_with(&replacement_name, &options).map_err(CoreError::io)?;
    replacement.write_all(expected).map_err(CoreError::io)?;
    replacement.sync_all().map_err(CoreError::io)?;
    drop(replacement);
    staging.rename(&replacement_name, &entry, "name.c9s").map_err(CoreError::io)?;
    sync_directory(&entry)?;
    sync_directory(&staging)?;
    mapping(&entry, expected)?;
    let (destination, name) = match action { A::Complete => (&target_parent, target_name), A::RestoreSource => (&parent, source_name) };
    rustix::fs::renameat_with(&staging, "entry", destination, name, rustix::fs::RenameFlags::NOREPLACE)
        .map_err(|e| CoreError::io(e.into()))?;
    sync_directory(destination)?;
    sync_directory(&staging)?;
    // Verify the public mapping after publication. A failure leaves it public
    // and NeedsAttention; no rollback or deletion of possibly edited data.
    mapping(&open_dir(destination, name)?, expected)?;
    // Leave staging (including earlier replacement fragments) for inspection.
    Ok(())
}
