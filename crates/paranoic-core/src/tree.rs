use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{CoreError, DirectoryEntry, EntryKind, ErrorCode, Location, Result, Storage};

/// Bounds apply to the complete traversal, including the starting directory.
#[derive(Clone, Copy, Debug)]
pub struct TreeLimits {
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_bytes: u64,
    pub max_path_bytes: usize,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self { max_entries: 100_000, max_depth: 128, max_bytes: 1 << 40, max_path_bytes: 32 * 1024 * 1024 }
    }
}

impl TreeLimits {
    pub fn validate(&self) -> Result<()> {
        if !(1..=1_000_000).contains(&self.max_entries)
            || !(1..=1024).contains(&self.max_depth)
            || self.max_bytes == 0 || self.max_bytes > i64::MAX as u64
            || !(4096..=256 * 1024 * 1024).contains(&self.max_path_bytes)
        {
            return Err(CoreError::new(ErrorCode::InvalidInput, "invalid tree traversal limits"));
        }
        Ok(())
    }
}

/// An inventory, NOT a filesystem snapshot or permission to delete later.
/// Parents precede children. Mutating consumers must revalidate their objects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeInventory {
    pub root: Location,
    pub entries: Vec<DirectoryEntry>,
    pub total_bytes: u64,
    pub files: u64,
    pub directories: u64,
}

pub(crate) fn scan(
    storage: &dyn Storage,
    root: &Location,
    limits: TreeLimits,
    directory_limit: usize,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<TreeInventory> {
    limits.validate()?;
    checkpoint()?;
    if root.storage != storage.id() || !storage.capabilities().read {
        return Err(CoreError::new(ErrorCode::PermissionDenied, "tree source grant is unavailable"));
    }
    let metadata = storage.metadata(&root.path)?;
    if metadata.kind != EntryKind::Directory {
        return Err(CoreError::new(ErrorCode::Unsupported, "tree source must be a directory"));
    }
    let mut inventory = TreeInventory {
        root: root.clone(), entries: vec![DirectoryEntry { location: root.clone(), metadata }],
        total_bytes: 0, files: 0, directories: 1,
    };
    let mut pending = vec![(root.path.clone(), 0_usize)];
    let mut seen = HashSet::new();
    seen.insert(root.path.clone());
    let mut path_bytes = root.path.as_str().len();
    while let Some((path, depth)) = pending.pop() {
        checkpoint()?;
        let before = storage.metadata(&path)?;
        if before.kind != EntryKind::Directory {
            return Err(CoreError::new(ErrorCode::SourceChanged, "directory changed during traversal"));
        }
        let remaining = limits.max_entries - inventory.entries.len();
        // A zero remaining budget still permits discovering an empty directory.
        let children = storage.list(&path, directory_limit.min(remaining.saturating_add(1)))?;
        if children.len() > remaining { return Err(limit()); }
        for entry in children {
            checkpoint()?;
            let (parent, _) = entry.location.path.parent_and_name()?;
            if entry.location.storage != root.storage || parent != path || !seen.insert(entry.location.path.clone()) {
                return Err(CoreError::new(ErrorCode::Integrity, "invalid directory listing from storage"));
            }
            path_bytes = path_bytes.checked_add(entry.location.path.as_str().len()).ok_or_else(limit)?;
            if path_bytes > limits.max_path_bytes || depth + 1 > limits.max_depth { return Err(limit()); }
            match entry.metadata.kind {
                EntryKind::Directory => {
                    inventory.directories += 1;
                    pending.push((entry.location.path.clone(), depth + 1));
                }
                EntryKind::File => {
                    inventory.files += 1;
                    inventory.total_bytes = inventory.total_bytes.checked_add(entry.metadata.bytes).ok_or_else(limit)?;
                    if inventory.total_bytes > limits.max_bytes { return Err(limit()); }
                }
                EntryKind::Symlink | EntryKind::Other => {
                    return Err(CoreError::new(ErrorCode::Unsupported, "tree contains a symlink or special entry"));
                }
            }
            inventory.entries.push(entry);
        }
        if storage.metadata(&path)? != before {
            return Err(CoreError::new(ErrorCode::SourceChanged, "directory changed during traversal"));
        }
    }
    checkpoint()?;
    Ok(inventory)
}

fn limit() -> CoreError {
    CoreError::new(ErrorCode::LimitExceeded, "directory tree exceeds traversal limits")
}
