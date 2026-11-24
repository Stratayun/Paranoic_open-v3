//! Cryptomator wrapper renames on cloud storage (roadmap 3.9): RenameMapped
//! and ReshapeDirectory with inspection and explicit recovery, mirroring the
//! local adapter's semantics with cloud primitives.
//!
//! The wrapper is first moved into a hidden ".paranoic-<job>" staging folder
//! as "entry" (one atomic relocate), its mapping is changed there, then it is
//! published with one atomic relocate. Clouds have no atomic replace, so a
//! mapping change never overwrites: the new mapping is uploaded beforehand as
//! "mapping-<job>", the old one is moved aside as "old-mapping-<job>" (kept as
//! evidence until the publication is verified, then the staging folder goes
//! to the provider trash), then the new one is moved in. Recovery understands every
//! intermediate state, including an entry momentarily without "name.c9s".
//! Requires `atomic_relocate` (Core checks the capability before running).
#![allow(clippy::too_many_arguments)] // mirrors the Storage trait signatures
use std::io::Read;
use paranoic_core::{DirectoryReshape, JobId, MappedEntryObservation as O, MappedRecoveryAction as A, MappedRenameObservation};
use crate::{CloudEntry, CloudId, CloudKind, CloudProvider, CoreError, ErrorCode, Result};

const LIMIT: u64 = 368;

fn integrity() -> CoreError { CoreError::new(ErrorCode::Integrity, "wrapper differs from request") }
fn staging_name(job: JobId) -> String { format!(".paranoic-{}", job.0.simple()) }

fn read_small(provider: &dyn CloudProvider, entry: &CloudEntry, limit: u64) -> Result<Vec<u8>> {
    if entry.kind != CloudKind::File || entry.bytes.is_none_or(|b| b > limit) { return Err(integrity()); }
    let mut bytes = Vec::new();
    provider.open_download(&entry.id, 0)?.take(limit + 1).read_to_end(&mut bytes).map_err(CoreError::io)?;
    if Some(bytes.len() as u64) != entry.bytes { return Err(CoreError::new(ErrorCode::SourceChanged, "wrapper file changed while reading")); }
    Ok(bytes)
}
fn child(provider: &dyn CloudProvider, folder: &CloudId, name: &str) -> Result<Option<CloudEntry>> { provider.lookup(folder, name) }
fn upload(provider: &dyn CloudProvider, folder: &CloudId, name: &str, bytes: &[u8]) -> Result<CloudEntry> {
    provider.upload_new(folder, name, bytes.len() as u64, &mut std::io::Cursor::new(bytes), &mut |_| Ok(()))
}
fn absent(provider: &dyn CloudProvider, folder: &CloudId, name: &str) -> Result<()> {
    if child(provider, folder, name)?.is_some() { Err(CoreError::new(ErrorCode::AlreadyExists, "destination exists")) } else { Ok(()) }
}

/// Mapping state of a wrapper folder (`name.c9s` vs old/new).
fn observe_mapping(provider: &dyn CloudProvider, folder: &CloudId, name: &str, old: &[u8], new: &[u8], check: &mut dyn FnMut() -> Result<()>) -> Result<O> {
    check()?;
    let Some(wrapper) = child(provider, folder, name)? else { return Ok(O::Missing) };
    if wrapper.kind != CloudKind::Folder { return Ok(O::Different); }
    let Some(mapping) = child(provider, &wrapper.id, "name.c9s")? else { return Ok(O::Different) };
    let bytes = match read_small(provider, &mapping, LIMIT) { Ok(b) => b, Err(e) if e.code == ErrorCode::Integrity => return Ok(O::Different), Err(e) => return Err(e) };
    check()?;
    Ok(match (bytes == old, bytes == new) { (true, true) => O::BothMappings, (true, false) => O::OldMapping, (false, true) => O::NewMapping, _ => O::Different })
}

/// Put `desired` as entry/name.c9s without ever overwriting; previous mapping
/// (if any) is preserved in staging as `old-mapping-<tag>`.
fn set_mapping(provider: &dyn CloudProvider, staging: &CloudId, entry: &CloudId, desired: Option<&[u8]>, tag: JobId) -> Result<()> {
    let fragment = match desired { Some(bytes) => Some(upload(provider, staging, &format!("mapping-{}", tag.0.simple()), bytes)?), None => None };
    if let Some(current) = child(provider, entry, "name.c9s")? {
        provider.relocate(&current, Some(staging), Some(&format!("old-mapping-{}", tag.0.simple())))?;
    }
    if let Some(fragment) = fragment {
        let fresh = provider.metadata(&fragment.id)?;
        provider.relocate(&fresh, Some(entry), Some("name.c9s"))?;
    }
    Ok(())
}
fn verify_mapping(provider: &dyn CloudProvider, entry: &CloudId, expected: &[u8]) -> Result<()> {
    let mapping = child(provider, entry, "name.c9s")?.ok_or_else(integrity)?;
    if read_small(provider, &mapping, LIMIT)? != expected { return Err(integrity()); }
    Ok(())
}

pub(crate) fn rename(provider: &dyn CloudProvider, parent: &CloudEntry, source: &str, target_parent: &CloudEntry, target: &str,
    job: JobId, old: &[u8], new: &[u8]) -> Result<()> {
    if old.is_empty() || new.is_empty() || old.len() as u64 > LIMIT || new.len() as u64 > LIMIT || (parent.id == target_parent.id && source == target) {
        return Err(CoreError::new(ErrorCode::InvalidInput, "invalid mapped rename"));
    }
    absent(provider, &target_parent.id, target)?;
    let wrapper = child(provider, &parent.id, source)?.ok_or_else(|| CoreError::new(ErrorCode::NotFound, "rename source missing"))?;
    if !matches!(observe_mapping(provider, &parent.id, source, old, new, &mut || Ok(()))?, O::OldMapping | O::BothMappings) { return Err(integrity()); }
    let staging = provider.create_folder(&parent.id, &staging_name(job))?;
    // From here the staging folder owns the only copy: never discard it on error.
    let entry = provider.relocate(&provider.metadata(&wrapper.id)?, Some(&staging.id), Some("entry"))?;
    set_mapping(provider, &staging.id, &entry.id, Some(new), job)?;
    verify_mapping(provider, &entry.id, new)?;
    provider.relocate(&provider.metadata(&entry.id)?, Some(&target_parent.id), Some(target))?;
    let published = child(provider, &target_parent.id, target)?.ok_or_else(|| CoreError::new(ErrorCode::NeedsAttention, "published wrapper not visible"))?;
    verify_mapping(provider, &published.id, new)?;
    retire_staging(provider, &staging.id);
    Ok(())
}

/// After a verified publication the staging folder holds only mapping
/// fragments (the old encrypted name). Like the local adapter, which removes
/// its empty staging, it is retired — here to the provider trash
/// (recoverable). Best effort: a failure leaves it for inspection, never
/// touching anything but mapping fragments.
fn retire_staging(provider: &dyn CloudProvider, staging: &CloudId) {
    let Ok(page) = provider.list(staging, None, 16) else { return };
    if page.next.is_some() || page.entries.iter().any(|e| e.kind != CloudKind::File
        || !(e.name.starts_with("mapping-") || e.name.starts_with("old-mapping-"))) { return; }
    for fragment in &page.entries { if provider.trash(fragment).is_err() { return; } }
    if let Ok(folder) = provider.metadata(staging) { let _ = provider.trash(&folder); }
}

pub(crate) fn inspect(provider: &dyn CloudProvider, parent: &CloudEntry, source: &str, target_parent: &CloudEntry, target: &str,
    job: JobId, old: &[u8], new: &[u8], check: &mut dyn FnMut() -> Result<()>) -> Result<MappedRenameObservation> {
    let source = observe_mapping(provider, &parent.id, source, old, new, check)?;
    let target = observe_mapping(provider, &target_parent.id, target, old, new, check)?;
    let staging = child(provider, &parent.id, &staging_name(job))?;
    let staged_entry = match &staging { Some(s) => observe_mapping(provider, &s.id, "entry", old, new, check)?, None => O::Missing };
    Ok(MappedRenameObservation { source, target, staged_entry, staging_present: staging.is_some() })
}

/// Choose a mapping fragment left in staging by an interrupted change.
fn fragment(provider: &dyn CloudProvider, staging: &CloudId, expected: &[u8]) -> Result<Option<CloudEntry>> {
    let mut page = None;
    loop {
        let chunk = provider.list(staging, page.as_deref(), 1000)?;
        for e in chunk.entries {
            if (e.name.starts_with("mapping-") || e.name.starts_with("old-mapping-")) && e.kind == CloudKind::File
                && read_small(provider, &e, LIMIT).is_ok_and(|b| b == expected) { return Ok(Some(e)); }
        }
        match chunk.next { Some(n) => page = Some(n), None => return Ok(None) }
    }
}

pub(crate) fn recover(provider: &dyn CloudProvider, parent: &CloudEntry, source: &str, target_parent: &CloudEntry, target: &str,
    original: JobId, recovery: JobId, old: &[u8], new: &[u8], action: A) -> Result<()> {
    if original == recovery || old.is_empty() || new.is_empty() || old.len() as u64 > LIMIT || new.len() as u64 > LIMIT {
        return Err(CoreError::new(ErrorCode::InvalidInput, "invalid mapped recovery request"));
    }
    absent(provider, &parent.id, source).map_err(|_| CoreError::new(ErrorCode::AlreadyExists, "public rename entry exists; recovery refused"))?;
    absent(provider, &target_parent.id, target).map_err(|_| CoreError::new(ErrorCode::AlreadyExists, "public rename entry exists; recovery refused"))?;
    let staging = child(provider, &parent.id, &staging_name(original))?.ok_or_else(|| CoreError::new(ErrorCode::NotFound, "rename staging missing"))?;
    let entry = child(provider, &staging.id, "entry")?.filter(|e| e.kind == CloudKind::Folder).ok_or_else(integrity)?;
    let expected = match action { A::Complete => new, A::RestoreSource => old };
    match child(provider, &entry.id, "name.c9s")? {
        Some(m) => {
            let bytes = read_small(provider, &m, LIMIT)?;
            if bytes != old && bytes != new { return Err(integrity()); }
            if bytes != expected { set_mapping(provider, &staging.id, &entry.id, Some(expected), recovery)?; }
        }
        None => {
            // Interrupted between moving the old mapping aside and moving the new one in.
            match fragment(provider, &staging.id, expected)? {
                Some(f) => { provider.relocate(&f, Some(&entry.id), Some("name.c9s"))?; }
                None => set_mapping(provider, &staging.id, &entry.id, Some(expected), recovery)?,
            }
        }
    }
    verify_mapping(provider, &entry.id, expected)?;
    let (destination, name) = match action { A::Complete => (target_parent, target), A::RestoreSource => (parent, source) };
    provider.relocate(&provider.metadata(&entry.id)?, Some(&destination.id), Some(name))?;
    let published = child(provider, &destination.id, name)?.ok_or_else(|| CoreError::new(ErrorCode::NeedsAttention, "recovered wrapper not visible"))?;
    verify_mapping(provider, &published.id, expected)
}

// ---------------- ReshapeDirectory ----------------

/// A directory wrapper holds exactly dir.c9r (== link) and optionally name.c9s (== mapping).
fn verify_wrapper(provider: &dyn CloudProvider, wrapper: &CloudId, link: &[u8], mapping: Option<&[u8]>) -> Result<()> {
    let listing = provider.list(wrapper, None, 3)?;
    if listing.next.is_some() || listing.entries.len() != if mapping.is_some() { 2 } else { 1 } { return Err(integrity()); }
    let dir = listing.entries.iter().find(|e| e.name == "dir.c9r").ok_or_else(integrity)?;
    if read_small(provider, dir, 64)? != link { return Err(integrity()); }
    if let Some(bytes) = mapping {
        let name = listing.entries.iter().find(|e| e.name == "name.c9s").ok_or_else(integrity)?;
        if read_small(provider, name, LIMIT)? != bytes { return Err(integrity()); }
    }
    Ok(())
}
fn classify(provider: &dyn CloudProvider, folder: &CloudId, name: &str, layout: &DirectoryReshape, check: &mut dyn FnMut() -> Result<()>) -> Result<O> {
    check()?;
    let Some(wrapper) = child(provider, folder, name)? else { return Ok(O::Missing) };
    if wrapper.kind != CloudKind::Folder { return Ok(O::Different); }
    let matches = |mapping: Option<&[u8]>| match verify_wrapper(provider, &wrapper.id, &layout.link, mapping) {
        Ok(()) => Ok(true), Err(e) if matches!(e.code, ErrorCode::Integrity | ErrorCode::NotFound) => Ok(false), Err(e) => Err(e) };
    let (old, new) = (matches(layout.old_mapping.as_deref())?, matches(layout.new_mapping.as_deref())?);
    check()?;
    Ok(match (old, new) { (true, true) => O::BothMappings, (true, false) => O::OldMapping, (false, true) => O::NewMapping, _ => O::Different })
}

pub(crate) fn reshape(provider: &dyn CloudProvider, parent: &CloudEntry, source: &str, target_parent: &CloudEntry, target: &str,
    job: JobId, layout: &DirectoryReshape) -> Result<()> {
    layout.validate()?;
    if parent.id == target_parent.id && source == target { return Err(CoreError::new(ErrorCode::InvalidInput, "invalid reshape destination")); }
    absent(provider, &target_parent.id, target)?;
    let wrapper = child(provider, &parent.id, source)?.ok_or_else(|| CoreError::new(ErrorCode::NotFound, "reshape source missing"))?;
    verify_wrapper(provider, &wrapper.id, &layout.link, layout.old_mapping.as_deref())?;
    let staging = provider.create_folder(&parent.id, &staging_name(job))?;
    let entry = provider.relocate(&provider.metadata(&wrapper.id)?, Some(&staging.id), Some("entry"))?;
    set_mapping(provider, &staging.id, &entry.id, layout.new_mapping.as_deref(), job)?;
    verify_wrapper(provider, &entry.id, &layout.link, layout.new_mapping.as_deref())?;
    provider.relocate(&provider.metadata(&entry.id)?, Some(&target_parent.id), Some(target))?;
    let published = child(provider, &target_parent.id, target)?.ok_or_else(|| CoreError::new(ErrorCode::NeedsAttention, "published wrapper not visible"))?;
    verify_wrapper(provider, &published.id, &layout.link, layout.new_mapping.as_deref())?;
    retire_staging(provider, &staging.id);
    Ok(())
}

pub(crate) fn inspect_reshape(provider: &dyn CloudProvider, parent: &CloudEntry, source: &str, target_parent: &CloudEntry, target: &str,
    job: JobId, layout: &DirectoryReshape, check: &mut dyn FnMut() -> Result<()>) -> Result<MappedRenameObservation> {
    layout.validate()?;
    let source = classify(provider, &parent.id, source, layout, check)?;
    let target = classify(provider, &target_parent.id, target, layout, check)?;
    let staging = child(provider, &parent.id, &staging_name(job))?;
    let staged_entry = match &staging { Some(s) => classify(provider, &s.id, "entry", layout, check)?, None => O::Missing };
    Ok(MappedRenameObservation { source, target, staged_entry, staging_present: staging.is_some() })
}

pub(crate) fn recover_reshape(provider: &dyn CloudProvider, parent: &CloudEntry, source: &str, target_parent: &CloudEntry, target: &str,
    original: JobId, recovery: JobId, layout: &DirectoryReshape, action: A) -> Result<()> {
    layout.validate()?;
    if original == recovery { return Err(CoreError::new(ErrorCode::InvalidInput, "invalid reshape recovery request")); }
    absent(provider, &parent.id, source).map_err(|_| CoreError::new(ErrorCode::AlreadyExists, "public reshape entry exists; recovery refused"))?;
    absent(provider, &target_parent.id, target).map_err(|_| CoreError::new(ErrorCode::AlreadyExists, "public reshape entry exists; recovery refused"))?;
    let staging = child(provider, &parent.id, &staging_name(original))?.ok_or_else(|| CoreError::new(ErrorCode::NotFound, "reshape staging is missing"))?;
    let entry = child(provider, &staging.id, "entry")?.filter(|e| e.kind == CloudKind::Folder).ok_or_else(integrity)?;
    let desired = match action { A::Complete => layout.new_mapping.as_deref(), A::RestoreSource => layout.old_mapping.as_deref() };
    if verify_wrapper(provider, &entry.id, &layout.link, desired).is_err() {
        // Convert from the other recognized layout (or an interrupted change).
        let other = match action { A::Complete => layout.old_mapping.as_deref(), A::RestoreSource => layout.new_mapping.as_deref() };
        let recognized = verify_wrapper(provider, &entry.id, &layout.link, other).is_ok()
            || verify_wrapper(provider, &entry.id, &layout.link, None).is_ok();
        if !recognized { return Err(integrity()); }
        match desired.and_then(|bytes| child(provider, &entry.id, "name.c9s").ok().flatten().is_none().then_some(bytes)) {
            Some(bytes) => match fragment(provider, &staging.id, bytes)? {
                Some(f) => { provider.relocate(&f, Some(&entry.id), Some("name.c9s"))?; }
                None => set_mapping(provider, &staging.id, &entry.id, Some(bytes), recovery)?,
            },
            None => set_mapping(provider, &staging.id, &entry.id, desired, recovery)?,
        }
    }
    verify_wrapper(provider, &entry.id, &layout.link, desired)?;
    let (destination, name) = match action { A::Complete => (target_parent, target), A::RestoreSource => (parent, source) };
    provider.relocate(&provider.metadata(&entry.id)?, Some(&destination.id), Some(name))?;
    let published = child(provider, &destination.id, name)?.ok_or_else(|| CoreError::new(ErrorCode::NeedsAttention, "recovered wrapper not visible"))?;
    verify_wrapper(provider, &published.id, &layout.link, desired)
}
