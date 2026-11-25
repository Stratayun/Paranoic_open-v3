//! Trash for device storage (owner's request). Deleting moves the entry into
//! `<root>/.Trash-Paranoic/<bucket>/<name>` with a no-replace move, so nothing
//! is erased until the owner empties the trash. The bucket name carries the
//! deletion time and the original folder (`<ms>-<rand>~<folder>`, folder
//! percent-encoded), so no separate index of paths is kept anywhere.
//! Only storages with an atomic no-replace move take part (local folders and
//! Android shared storage where the kernel honours it); others delete as before.

use paranoic_core::{EntryKind, ResourcePath, StorageId};
use uuid::Uuid;

use crate::{features::now_ms, BridgeError, NativeLocation, NativeRuntime, Result};

pub(crate) const TRASH: &str = ".Trash-Paranoic";
const MAX_FOLDER_SUFFIX: usize = 180;
const MAX_ITEMS: usize = 10_000;

/// One trashed entry.
#[derive(uniffi::Record, Clone, Debug)]
pub struct NativeTrashItem {
    /// Opaque id inside the trash (the bucket name).
    pub bucket: String,
    pub name: String,
    /// Folder the entry came from ("" = root); restore goes back there.
    pub original_folder: String,
    pub deleted_ms: u64,
    pub folder: bool,
    pub bytes: u64,
}

fn encode(folder: &str) -> String {
    folder.replace('%', "%25").replace('/', "%2F").replace('~', "%7E")
}
fn decode(text: &str) -> Option<String> {
    let out = text.replace("%2F", "/").replace("%7E", "~").replace("%25", "%");
    (out.is_empty() || ResourcePath::parse(out.clone()).is_ok()).then_some(out)
}
/// (deleted_ms, original folder) from a bucket name.
fn parse_bucket(bucket: &str) -> Option<(u64, String)> {
    let (head, folder) = match bucket.split_once('~') { Some((h, f)) => (h, decode(f)?), None => (bucket, String::new()) };
    let (ms, _) = head.split_once('-')?;
    Some((ms.parse().ok()?, folder))
}

#[uniffi::export]
impl NativeRuntime {
    /// Whether deleting in this place goes to the trash.
    pub fn trash_supported(&self, storage_id: String) -> bool {
        self.trash_storage(&storage_id).is_ok()
    }

    /// Move an entry into the trash of its place. Never replaces anything.
    pub fn trash_put(&self, location: NativeLocation) -> Result<()> {
        let location = location.core()?;
        if location.path.is_root() || location.path.as_str().split('/').next() == Some(TRASH) { return Err(BridgeError::Invalid); }
        let storage = self.trash_storage(&location.storage.0.to_string())?;
        let (parent, name) = location.path.parent_and_name()?;
        let trash = ResourcePath::parse(TRASH)?;
        match storage.create_directory(&trash) {
            Ok(()) => {}
            Err(e) if e.code == paranoic_core::ErrorCode::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let folder = encode(parent.as_str());
        let id = Uuid::new_v4().simple().to_string();
        let bucket = if folder.is_empty() { format!("{}-{}", now_ms(), &id[..8]) }
            else if folder.len() <= MAX_FOLDER_SUFFIX { format!("{}-{}~{folder}", now_ms(), &id[..8]) }
            else { return Err(BridgeError::Limit) };
        let bucket = trash.join(&bucket)?;
        storage.create_directory(&bucket)?;
        let target = bucket.join(name)?;
        if let Err(error) = storage.move_no_replace(&location.path, &target) {
            let _ = storage.delete_empty_directory(&bucket);
            return Err(error.into());
        }
        Ok(())
    }

    /// Entries in the trash of a place, newest first.
    pub fn trash_list(&self, storage_id: String) -> Result<Vec<NativeTrashItem>> {
        let storage = self.trash_storage(&storage_id)?;
        let trash = ResourcePath::parse(TRASH)?;
        let buckets = match storage.list(&trash, MAX_ITEMS) {
            Ok(list) => list,
            Err(e) if e.code == paranoic_core::ErrorCode::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for bucket in buckets {
            if bucket.metadata.kind != EntryKind::Directory { continue; }
            let Ok((_, bucket_name)) = bucket.location.path.parent_and_name() else { continue };
            let Some((deleted_ms, original_folder)) = parse_bucket(bucket_name) else { continue };
            let Ok(inside) = storage.list(&bucket.location.path, 2) else { continue };
            let [entry] = inside.as_slice() else { continue };
            let Ok((_, name)) = entry.location.path.parent_and_name() else { continue };
            out.push(NativeTrashItem { bucket: bucket_name.to_owned(), name: name.to_owned(), original_folder, deleted_ms,
                folder: entry.metadata.kind == EntryKind::Directory, bytes: entry.metadata.bytes });
        }
        out.sort_by(|a, b| b.deleted_ms.cmp(&a.deleted_ms));
        Ok(out)
    }

    /// Put an entry back where it was. Conflict if that name is taken now;
    /// NotFound-like Invalid if the original folder is gone.
    pub fn trash_restore(&self, storage_id: String, bucket: String) -> Result<()> {
        let storage = self.trash_storage(&storage_id)?;
        let (bucket_path, item) = self.trash_item(&storage_id, &bucket)?;
        let (_, original) = parse_bucket(&bucket).ok_or(BridgeError::Invalid)?;
        let folder = if original.is_empty() { ResourcePath::root() } else { ResourcePath::parse(original)? };
        storage.move_no_replace(&bucket_path.join(&item.name)?, &folder.join(&item.name)?)?;
        let _ = storage.delete_empty_directory(&bucket_path);
        Ok(())
    }

    /// Erase one trashed entry for good.
    pub fn trash_purge(&self, storage_id: String, bucket: String) -> Result<()> {
        let storage = self.trash_storage(&storage_id)?;
        let (bucket_path, _) = self.trash_item(&storage_id, &bucket)?;
        let mut budget = 100_000usize;
        remove_tree(storage.as_ref(), &bucket_path, &mut budget)
    }

    /// Erase everything in the trash of a place. Returns how many entries went.
    pub fn trash_empty(&self, storage_id: String) -> Result<u32> {
        let items = self.trash_list(storage_id.clone())?;
        let mut removed = 0;
        for item in items { self.trash_purge(storage_id.clone(), item.bucket)?; removed += 1; }
        Ok(removed)
    }
}

impl NativeRuntime {
    fn trash_storage(&self, storage_id: &str) -> Result<std::sync::Arc<dyn paranoic_core::Storage>> {
        let id = StorageId(Uuid::parse_str(storage_id).map_err(|_| BridgeError::Invalid)?);
        let storage = self.core.storage(id)?;
        let caps = storage.capabilities();
        if storage.domain() != paranoic_core::StorageDomain::Files || !caps.write || !caps.delete || !caps.atomic_move_no_replace {
            return Err(BridgeError::Unsupported);
        }
        if self.grants.lock().map_err(crate::lock_error)?.iter().any(|g| g.id == storage_id && (g.android_tree || crate::features::is_cloud(g))) {
            return Err(BridgeError::Unsupported);
        }
        Ok(storage)
    }
    /// The bucket path and its single entry; the bucket must be a plain child of the trash.
    fn trash_item(&self, storage_id: &str, bucket: &str) -> Result<(ResourcePath, NativeTrashItem)> {
        if bucket.is_empty() || bucket.contains('/') || parse_bucket(bucket).is_none() { return Err(BridgeError::Invalid); }
        let item = self.trash_list(storage_id.to_owned())?.into_iter().find(|i| i.bucket == bucket).ok_or(BridgeError::Changed)?;
        Ok((ResourcePath::parse(TRASH)?.join(bucket)?, item))
    }
}

/// Depth-first removal below the trash only; symlinks are removed, never followed.
fn remove_tree(storage: &dyn paranoic_core::Storage, path: &ResourcePath, budget: &mut usize) -> Result<()> {
    if path.as_str().split('/').next() != Some(TRASH) { return Err(BridgeError::Invalid); }
    let metadata = storage.metadata(path)?;
    if metadata.kind == EntryKind::Directory {
        loop {
            let children = storage.list(path, 1000)?;
            if children.is_empty() { break; }
            for child in children {
                *budget = budget.checked_sub(1).ok_or(BridgeError::Limit)?;
                remove_tree(storage, &child.location.path, budget)?;
            }
        }
        storage.delete_empty_directory(path)?;
    } else {
        storage.delete_file(path)?;
    }
    Ok(())
}

