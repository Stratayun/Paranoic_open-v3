//! Retain a directory wrapper while changing its shortening class.
use super::*;
use cap_std::fs::{DirBuilderExt, OpenOptionsExt, MetadataExt};
use paranoic_core::DirectoryReshape;

fn directory(parent: &Dir, name: &str) -> Result<Dir> {
    let fd = rustix::fs::openat(parent, name, rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY |
        rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC, rustix::fs::Mode::empty())
        .map_err(|e| CoreError::io(e.into()))?;
    Ok(Dir::from_std_file(std::fs::File::from(fd)))
}
fn exact_file(dir: &Dir, name: &str, expected: &[u8]) -> Result<()> {
    let fd = rustix::fs::openat(dir, name, rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW |
        rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC, rustix::fs::Mode::empty())
        .map_err(|e| CoreError::io(e.into()))?;
    let file = std::fs::File::from(fd);
    if !file.metadata().map_err(CoreError::io)?.is_file() { return Err(integrity()); }
    let mut bytes = Vec::new();
    file.take(expected.len() as u64 + 1).read_to_end(&mut bytes).map_err(CoreError::io)?;
    if bytes != expected { return Err(integrity()); }
    Ok(())
}
fn integrity() -> CoreError { CoreError::new(ErrorCode::Integrity, "directory wrapper differs from request") }
fn verify(dir: &Dir, link: &[u8], mapping: Option<&[u8]>) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for entry in dir.entries().map_err(CoreError::io)? {
        let name = entry.map_err(CoreError::io)?.file_name();
        if names.len() >= 2 || !names.insert(name) { return Err(integrity()); }
    }
    if names.len() != if mapping.is_some() { 2 } else { 1 }
        || !names.contains(std::ffi::OsStr::new("dir.c9r")) { return Err(integrity()); }
    exact_file(dir, "dir.c9r", link)?;
    if let Some(bytes) = mapping { exact_file(dir, "name.c9s", bytes)?; }
    Ok(())
}
pub(super) fn rename(storage: &LocalStorage, source: &ResourcePath, target: &ResourcePath,
    job: JobId, layout: &DirectoryReshape) -> Result<()> {
    storage.writable()?;
    layout.validate()?;
    if source == target || target.as_str().starts_with(&format!("{}/", source.as_str())) {
        return Err(CoreError::new(ErrorCode::InvalidInput, "invalid reshape destination"));
    }
    let (parent, source_name) = storage.parent(source)?;
    let (destination, target_name) = storage.parent(target)?;
    match destination.symlink_metadata(target_name) {
        Ok(_) => return Err(CoreError::new(ErrorCode::AlreadyExists, "reshape destination exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(CoreError::io(e)),
    }
    verify(&directory(&parent, source_name)?, &layout.link, layout.old_mapping.as_deref())?;
    let temporary = format!(".paranoic-{}", job.0);
    let mut builder = DirBuilder::new(); builder.mode(0o700);
    parent.create_dir_with(&temporary, &builder).map_err(CoreError::io)?;
    let staging = directory(&parent, &temporary)?;
    let metadata = staging.dir_metadata().map_err(CoreError::io)?;
    if !crate::shared::staging_private(&metadata, storage.shared_root) {
        return Err(CoreError::new(ErrorCode::PermissionDenied, "reshape staging is not private"));
    }
    sync_directory(&parent)?;
    crate::shared::rename_noreplace(&parent, source_name, &staging, "entry", storage.shared_root)
        .map_err(|e| CoreError::io(e.into()))?;
    sync_directory(&staging)?; sync_directory(&parent)?;
    // Never recursively clean this directory: it now owns the sole wrapper.
    let entry = directory(&staging, "entry")?;
    verify(&entry, &layout.link, layout.old_mapping.as_deref())?;
    if let Some(bytes) = &layout.new_mapping {
        let mut options = OpenOptions::new(); options.write(true).create_new(true).mode(0o600);
        let mut file = entry.open_with("name.c9s", &options).map_err(CoreError::io)?;
        file.write_all(bytes).map_err(CoreError::io)?;
        file.sync_all().map_err(CoreError::io)?;
    } else {
        // Preserve the former mapping as recovery evidence instead of unlinking.
        crate::shared::rename_noreplace(&entry, "name.c9s", &staging, "old-mapping", storage.shared_root)
            .map_err(|e| CoreError::io(e.into()))?;
    }
    sync_directory(&entry)?; sync_directory(&staging)?;
    verify(&entry, &layout.link, layout.new_mapping.as_deref())?;
    crate::shared::rename_noreplace(&staging, "entry", &destination, target_name, storage.shared_root)
        .map_err(|e| CoreError::io(e.into()))?;
    sync_directory(&destination)?; sync_directory(&staging)?;
    verify(&directory(&destination, target_name)?, &layout.link, layout.new_mapping.as_deref())
}

/// Classify a wrapper against the old/new layouts without modifying it.
fn classify(parent: &Dir, name: &str, layout: &DirectoryReshape, check: &mut dyn FnMut() -> Result<()>)
    -> Result<paranoic_core::MappedEntryObservation> {
    use paranoic_core::MappedEntryObservation as O;
    check()?;
    let before = match parent.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(O::Missing),
        Err(e) => return Err(CoreError::io(e)),
    };
    if !before.is_dir() { return Ok(O::Different); }
    let held = directory(parent, name)?;
    let matches = |mapping: Option<&[u8]>| match verify(&held, &layout.link, mapping) {
        Ok(()) => Ok(true),
        Err(error) if matches!(error.code, ErrorCode::Integrity | ErrorCode::NotFound) => Ok(false),
        Err(error) => Err(error),
    };
    let old = matches(layout.old_mapping.as_deref())?;
    let new = matches(layout.new_mapping.as_deref())?;
    check()?;
    let after = parent.symlink_metadata(name).map_err(CoreError::io)?;
    let metadata = held.dir_metadata().map_err(CoreError::io)?;
    if before.dev() != metadata.dev() || before.ino() != metadata.ino() || after.dev() != metadata.dev() || after.ino() != metadata.ino() {
        return Err(CoreError::new(ErrorCode::SourceChanged, "reshape entry changed during inspection"));
    }
    Ok(match (old, new) { (true, true) => O::BothMappings, (true, false) => O::OldMapping,
        (false, true) => O::NewMapping, _ => O::Different })
}

fn private_staging(parent: &Dir, job: JobId, shared: bool) -> Result<Option<Dir>> {
    let name = format!(".paranoic-{}", job.0);
    let staging = match directory(parent, &name) {
        Ok(dir) => dir,
        Err(e) if e.code == ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let metadata = staging.dir_metadata().map_err(CoreError::io)?;
    let visible = parent.symlink_metadata(&name).map_err(CoreError::io)?;
    if !crate::shared::staging_private(&metadata, shared)
        || visible.dev() != metadata.dev() || visible.ino() != metadata.ino() {
        return Err(CoreError::new(ErrorCode::PermissionDenied, "reshape staging is not private"));
    }
    Ok(Some(staging))
}

pub(super) fn inspect(storage: &LocalStorage, source: &ResourcePath, target: &ResourcePath, job: JobId,
    layout: &DirectoryReshape, check: &mut dyn FnMut() -> Result<()>) -> Result<paranoic_core::MappedRenameObservation> {
    use paranoic_core::{MappedEntryObservation as O, MappedRenameObservation};
    layout.validate()?;
    check()?;
    let (parent, name) = storage.parent(source)?;
    let (destination, target_name) = storage.parent(target)?;
    let source = classify(&parent, name, layout, check)?;
    let target = classify(&destination, target_name, layout, check)?;
    let staging = private_staging(&parent, job, storage.shared_root)?;
    let staged_entry = match &staging { Some(dir) => classify(dir, "entry", layout, check)?, None => O::Missing };
    check()?;
    Ok(MappedRenameObservation { source, target, staged_entry, staging_present: staging.is_some() })
}

/// Publish the retained wrapper of `original` after an explicit decision.
/// The staged wrapper is converted to the requested layout using names unique
/// to `recovery`; a removed mapping is preserved in staging, never unlinked.
pub(super) fn recover(storage: &LocalStorage, source: &ResourcePath, target: &ResourcePath,
    original: JobId, recovery: JobId, layout: &DirectoryReshape, action: paranoic_core::MappedRecoveryAction) -> Result<()> {
    use paranoic_core::{MappedEntryObservation as O, MappedRecoveryAction as A};
    storage.writable()?;
    layout.validate()?;
    if original == recovery || source == target {
        return Err(CoreError::new(ErrorCode::InvalidInput, "invalid reshape recovery request"));
    }
    let (parent, source_name) = storage.parent(source)?;
    let (target_parent, target_name) = storage.parent(target)?;
    for (dir, name) in [(&parent, source_name), (&target_parent, target_name)] {
        match dir.symlink_metadata(name) {
            Ok(_) => return Err(CoreError::new(ErrorCode::AlreadyExists, "public reshape entry exists; recovery refused")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(CoreError::io(e)),
        }
    }
    let staging = private_staging(&parent, original, storage.shared_root)?
        .ok_or_else(|| CoreError::new(ErrorCode::NotFound, "reshape staging is missing"))?;
    let state = classify(&staging, "entry", layout, &mut || Ok(()))?;
    let (desired, destination, name) = match action {
        A::Complete => (layout.new_mapping.as_deref(), &target_parent, target_name),
        A::RestoreSource => (layout.old_mapping.as_deref(), &parent, source_name),
    };
    let current_is_desired = match (state, action) {
        (O::NewMapping, A::Complete) | (O::OldMapping, A::RestoreSource) => true,
        (O::OldMapping, A::Complete) | (O::NewMapping, A::RestoreSource) => false,
        _ => return Err(CoreError::new(ErrorCode::Integrity, "staged wrapper has no recognized layout")),
    };
    let entry = directory(&staging, "entry")?;
    if !current_is_desired {
        match desired {
            Some(bytes) => {
                let fragment = format!("mapping-{}", recovery.0);
                let mut options = OpenOptions::new(); options.write(true).create_new(true).mode(0o600);
                let mut file = staging.open_with(&fragment, &options).map_err(CoreError::io)?;
                file.write_all(bytes).map_err(CoreError::io)?;
                file.sync_all().map_err(CoreError::io)?;
                drop(file);
                crate::shared::rename_noreplace(&staging, &fragment, &entry, "name.c9s", storage.shared_root)
                    .map_err(|e| CoreError::io(e.into()))?;
            }
            None => {
                let preserved = format!("old-mapping-{}", recovery.0);
                crate::shared::rename_noreplace(&entry, "name.c9s", &staging, &preserved, storage.shared_root)
                    .map_err(|e| CoreError::io(e.into()))?;
            }
        }
        sync_directory(&entry)?; sync_directory(&staging)?;
    }
    verify(&entry, &layout.link, desired)?;
    crate::shared::rename_noreplace(&staging, "entry", destination, name, storage.shared_root)
        .map_err(|e| CoreError::io(e.into()))?;
    sync_directory(destination)?; sync_directory(&staging)?;
    // Staging (with any preserved mapping) is left for inspection.
    verify(&directory(destination, name)?, &layout.link, desired)
}
