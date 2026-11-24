//! Fresh, read-only evidence for bounded bootstrap publication.
use std::{collections::BTreeMap, io::Read};
use crate::{CoreConfig, CoreError, EntryKind, ErrorCode, Location, Metadata,
    PreparedTreeDifference as Difference, PreparedTreeFile, PreparedTreeObservation as Observation,
    ResourcePath, Result, Storage};

type Layout = BTreeMap<String, BTreeMap<String, EntryKind>>;

fn differs(path: ResourcePath, reason: Difference) -> Observation {
    Observation::Differs { path, reason }
}

struct Plan { layout: Layout, paths: Vec<ResourcePath>, list_limit: usize }

pub(crate) fn validate(target: &Location, files: &[PreparedTreeFile], config: &CoreConfig) -> Result<()> {
    plan(target, files, config).map(|_| ())
}

fn plan(target: &Location, files: &[PreparedTreeFile], config: &CoreConfig) -> Result<Plan> {
    crate::validate_prepared_tree(files)?;
    if target.path.is_root() {
        return Err(CoreError::new(ErrorCode::InvalidInput, "invalid bootstrap inspection target"));
    }
    // Validate every path and budget before reading external storage.
    let mut layout: Layout = BTreeMap::new();
    let mut paths = Vec::with_capacity(files.len());
    for file in files {
        if file.bytes.len() as u64 > config.max_file_bytes {
            return Err(CoreError::new(ErrorCode::LimitExceeded, "bootstrap inspection exceeds file budget"));
        }
        let mut parent = target.path.as_str().to_owned();
        let parts: Vec<_> = file.path.as_str().split('/').collect();
        for (index, component) in parts.iter().enumerate() {
            let child = ResourcePath::parse(format!("{parent}/{component}"))?;
            let kind = if index + 1 == parts.len() { EntryKind::File } else { EntryKind::Directory };
            layout.entry(parent).or_default().insert(child.as_str().to_owned(), kind);
            parent = child.as_str().to_owned();
        }
        paths.push(ResourcePath::parse(parent)?);
    }
    let list_limit = config.max_list_entries.min(256);
    if layout.values().any(|children| children.len() > list_limit) {
        return Err(CoreError::new(ErrorCode::LimitExceeded, "bootstrap inspection exceeds listing budget"));
    }
    Ok(Plan { layout, paths, list_limit })
}

pub(crate) fn inspect(storage: &dyn Storage, target: &Location, files: &[PreparedTreeFile],
    config: &CoreConfig, checkpoint: &mut dyn FnMut(u64) -> Result<()>) -> Result<Observation> {
    let Plan { layout, paths, list_limit } = plan(target, files, config)?;
    if target.storage != storage.id() || !storage.capabilities().read {
        return Err(CoreError::new(ErrorCode::PermissionDenied, "bootstrap inspection storage unavailable"));
    }
    checkpoint(0)?;
    let root = match storage.metadata(&target.path) {
        Ok(metadata) => metadata,
        Err(error) if error.code == ErrorCode::NotFound => {
            checkpoint(0)?;
            return Ok(Observation::Missing);
        }
        Err(error) => return Err(error),
    };
    if root.kind != EntryKind::Directory {
        return Ok(differs(target.path.clone(), Difference::EntryKind));
    }
    let mut observed = BTreeMap::new();
    observed.insert(target.path.as_str().to_owned(), root);
    if let Some(difference) = inspect_layout(storage, &layout, list_limit, &mut observed,
        &mut || checkpoint(0))? { return Ok(difference); }

    // Bootstrap payloads are at most 64 KiB; never allocate a large transfer
    // buffer or trust a provider's advertised size as an allocation request.
    let mut buffer = [0_u8; 8192];
    let mut done = 0_u64;
    for (file, path) in files.iter().zip(paths) {
        checkpoint(done)?;
        let mut reader = match storage.open_read(&path) {
            Ok(reader) => reader,
            Err(error) if error.code == ErrorCode::NotFound => return Ok(differs(path, Difference::MissingEntry)),
            Err(error) => return Err(error),
        };
        let before = reader.metadata()?;
        if before.kind != EntryKind::File { return Ok(differs(path, Difference::EntryKind)); }
        if before.bytes != file.bytes.len() as u64 { return Ok(differs(path, Difference::Contents)); }
        if observed.get(path.as_str()) != Some(&before) { return Ok(differs(path, Difference::Changed)); }
        let mut total = 0_u64;
        let mut hash = blake3::Hasher::new();
        loop {
            checkpoint(done)?;
            let count = match reader.read(&mut buffer) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => result.map_err(CoreError::io)?,
            };
            if count == 0 { break; }
            total = total.checked_add(count as u64).ok_or_else(CoreError::internal)?;
            if total > before.bytes { return Ok(differs(path, Difference::Contents)); }
            hash.update(&buffer[..count]);
        }
        if total != before.bytes || hash.finalize() != blake3::hash(&file.bytes) {
            return Ok(differs(path, Difference::Contents));
        }
        if reader.metadata()? != before { return Ok(differs(path, Difference::Changed)); }
        done += total;
        checkpoint(done)?;
    }
    // Reobserve directory membership and metadata after reading all files.
    // This detects ordinary concurrent changes, but is not a storage snapshot.
    if let Some(difference) = inspect_layout(storage, &layout, list_limit, &mut observed,
        &mut || checkpoint(done))? { return Ok(difference); }
    checkpoint(done)?;
    Ok(Observation::Matches)
}

fn inspect_layout(storage: &dyn Storage, layout: &Layout, limit: usize,
    observed: &mut BTreeMap<String, Metadata>, checkpoint: &mut dyn FnMut() -> Result<()>)
    -> Result<Option<Observation>> {
    for (directory, children) in layout {
        checkpoint()?;
        let path = ResourcePath::parse(directory.clone())?;
        let before = match storage.metadata(&path) {
            Ok(value) => value,
            Err(error) if error.code == ErrorCode::NotFound => return Ok(Some(differs(path, Difference::MissingEntry))),
            Err(error) => return Err(error),
        };
        if before.kind != EntryKind::Directory { return Ok(Some(differs(path, Difference::EntryKind))); }
        if let Some(previous) = observed.insert(directory.clone(), before.clone()) {
            if previous != before { return Ok(Some(differs(path, Difference::Changed))); }
        }
        let rows = storage.list(&path, limit)?;
        if rows.len() > limit {
            return Err(CoreError::new(ErrorCode::LimitExceeded, "provider exceeded bootstrap listing budget"));
        }
        let mut actual = BTreeMap::new();
        for row in rows {
            checkpoint()?;
            if row.location.storage != storage.id() || row.location.path.parent_and_name()?.0 != path
                || actual.insert(row.location.path.as_str().to_owned(), row.metadata.kind).is_some() {
                return Err(CoreError::new(ErrorCode::Integrity, "invalid bootstrap listing"));
            }
            let key = row.location.path.as_str().to_owned();
            if let Some(previous) = observed.insert(key, row.metadata.clone()) {
                if previous != row.metadata {
                    return Ok(Some(differs(row.location.path, Difference::Changed)));
                }
            }
        }
        if &actual != children { return Ok(Some(differs(path, Difference::DirectoryEntries))); }
        if storage.metadata(&path)? != before { return Ok(Some(differs(path, Difference::Changed))); }
    }
    Ok(None)
}
