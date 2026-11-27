//! Trash for device storage (owner's request). Deleting moves the entry into
//! `<root>/.Trash-Paranoic/<bucket>/<name>` with a no-replace move, so nothing
//! is erased until the owner empties the trash. The bucket is `<ms>-<rand>`;
//! the original folder is in a small `.origin` file beside the entry (owner
//! 26.11: deep paths no longer hit the name length limit). Buckets of the
//! earlier `<ms>-<rand>~<folder>` form are still read.
//! Only storages with an atomic no-replace move take part (local folders and
//! Android shared storage where the kernel honours it); others delete as before.

use paranoic_core::{EntryKind, ResourcePath, StorageId};
use uuid::Uuid;

use crate::{features::now_ms, BridgeError, NativeLocation, NativeRuntime, Result};

pub(crate) const TRASH: &str = ".Trash-Paranoic";
const ORIGIN: &str = ".origin";
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
        if name == ORIGIN { return Err(BridgeError::Invalid); }
        let id = Uuid::new_v4().simple().to_string();
        let bucket = trash.join(&format!("{}-{}", now_ms(), &id[..8]))?;
        storage.create_directory(&bucket)?;
        // Where it came from, written first: an entry is never in the trash without it.
        let origin = bucket.join(ORIGIN)?;
        let written = (|| -> paranoic_core::Result<()> {
            let mut sink = storage.stage(&origin, paranoic_core::JobId(Uuid::new_v4()))?;
            if let Err(e) = std::io::Write::write_all(&mut sink, parent.as_str().as_bytes()) { let _ = sink.abort(); return Err(paranoic_core::CoreError::io(e)); }
            sink.publish()
        })();
        if let Err(error) = written {
            let _ = storage.delete_empty_directory(&bucket);
            return Err(error.into());
        }
        let target = bucket.join(name)?;
        if let Err(error) = storage.move_no_replace(&location.path, &target) {
            let _ = storage.delete_file(&origin);
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
            let Some((deleted_ms, mut original_folder)) = parse_bucket(bucket_name) else { continue };
            let Ok(inside) = storage.list(&bucket.location.path, 3) else { continue };
            let entries: Vec<_> = inside.iter().filter(|e| e.location.path.parent_and_name().map(|(_, n)| n != ORIGIN).unwrap_or(false)).collect();
            let [entry] = entries.as_slice() else { continue };
            if !bucket_name.contains('~') { original_folder = read_origin(storage.as_ref(), &bucket.location.path).unwrap_or_default(); }
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
        // Audit A07: a missing or damaged `.origin` is not "the root"; restore refuses instead of
        // putting the entry somewhere it never was.
        let original = if bucket.contains('~') { item.original_folder.clone() } else {
            read_origin(storage.as_ref(), &bucket_path).ok_or(BridgeError::Invalid)?
        };
        let folder = if original.is_empty() { ResourcePath::root() } else { ResourcePath::parse(original)? };
        storage.move_no_replace(&bucket_path.join(&item.name)?, &folder.join(&item.name)?)?;
        let _ = storage.delete_file(&bucket_path.join(ORIGIN)?);
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

#[uniffi::export]
impl NativeRuntime {
    /// Cleanup (owner 26.11): BLAKE3 of a file's bytes, hex, to group exact duplicates.
    /// Read through Core with the metadata pinned before and after.
    pub fn file_hash(&self, location: NativeLocation) -> Result<String> {
        let location = location.core()?;
        let metadata = self.core.metadata(&location)?;
        if metadata.kind != EntryKind::File { return Err(BridgeError::Invalid); }
        let digest = self.content_digest(&location, &metadata)?;
        Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Cleanup pre-filter: BLAKE3 of the size, the first and the last 64 KiB.
    /// Equal samples only make a full `file_hash` worthwhile; different samples
    /// prove the files differ. Without random access the tail is read through.
    pub fn file_sample_hash(&self, location: NativeLocation) -> Result<String> {
        use std::io::Read;
        const SAMPLE: u64 = 64 * 1024;
        let location = location.core()?;
        let metadata = self.core.metadata(&location)?;
        if metadata.kind != EntryKind::File { return Err(BridgeError::Invalid); }
        let mut input = self.core.open_read(&location)?;
        if input.metadata()? != metadata { return Err(BridgeError::Changed); }
        let mut hash = blake3::Hasher::new();
        hash.update(&metadata.bytes.to_le_bytes());
        let mut buffer = vec![0u8; SAMPLE as usize];
        let mut read_exact = |input: &mut Box<dyn paranoic_core::ReadHandle>, len: usize, hash: &mut blake3::Hasher| -> Result<()> {
            let mut filled = 0;
            while filled < len {
                let n = input.read(&mut buffer[filled..len]).map_err(paranoic_core::CoreError::io)?;
                if n == 0 { return Err(BridgeError::Changed); }
                filled += n;
            }
            hash.update(&buffer[..len]);
            Ok(())
        };
        let head = metadata.bytes.min(SAMPLE);
        read_exact(&mut input, head as usize, &mut hash)?;
        if metadata.bytes > SAMPLE {
            let tail_start = metadata.bytes.saturating_sub(SAMPLE).max(SAMPLE);
            if input.seek_to(tail_start).is_err() {
                let mut skip = tail_start - head;
                let mut scratch = vec![0u8; 256 * 1024];
                while skip > 0 {
                    let n = input.read(&mut scratch[..skip.min(256 * 1024) as usize]).map_err(paranoic_core::CoreError::io)?;
                    if n == 0 { return Err(BridgeError::Changed); }
                    skip -= n as u64;
                }
            }
            read_exact(&mut input, (metadata.bytes - tail_start) as usize, &mut hash)?;
        }
        if input.metadata()? != metadata { return Err(BridgeError::Changed); }
        Ok(hash.finalize().to_hex().to_string())
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

/// The original folder recorded in a bucket's `.origin` (bounded, UTF-8, a valid path).
fn read_origin(storage: &dyn paranoic_core::Storage, bucket: &ResourcePath) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    storage.open_read(&bucket.join(ORIGIN).ok()?).ok()?.take(4097).read_to_string(&mut text).ok()?;
    (text.len() <= 4096 && (text.is_empty() || ResourcePath::parse(text.clone()).is_ok())).then_some(text)
}

/// Depth-first removal below the trash only; symlinks are removed, never followed.
fn remove_tree(storage: &dyn paranoic_core::Storage, path: &ResourcePath, budget: &mut usize) -> Result<()> {
    if path.as_str().split('/').next() != Some(TRASH) { return Err(BridgeError::Invalid); }
    let metadata = storage.metadata(path)?;
    if metadata.kind == EntryKind::Directory {
        // Audit A05: `list` fails past its limit instead of returning a page, so ask for the
        // whole remaining budget at once (bounded by the overall trash budget).
        loop {
            let children = storage.list(path, (*budget).clamp(1, 100_000))?;
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

