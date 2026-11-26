//! Android SAF grant as a Core `Storage`. Kotlin implements only document
//! traversal and descriptor IO; every policy decision stays here.
//!
//! Create-new protocol (SAF has no atomic "create if absent"):
//! 1. the target name must be absent;
//! 2. bytes go to a new hidden document ".paranoic-<job>" in the same folder;
//! 3. publish = rename to the target name; providers that find the name taken
//!    pick a unique name instead (AOSP buildUniqueFile), so the actual name is
//!    checked and, if it differs, our own document is deleted → AlreadyExists.
//!
//! Writes are enabled only for the AOSP external-storage provider, whose
//! create/rename choose a unique name instead of overwriting; other
//! providers are read-only. Residual: inside that provider the unique-name
//! choice and the rename are separate steps (tiny window against another app
//! creating the same name). The temporary document is addressed by its URI
//! from creation on, never re-resolved by path. Moves are not offered (no
//! atomic no-replace). Deletion is offered only with a host-confirmed native
//! unlink/rmdir path; generic deleteDocument is never used for cleanup either.
use std::{io::{self, Read, Write}, sync::Arc};
use paranoic_core::*;
use crate::{AndroidDocuments, HostInfo};

pub(crate) struct SafStorage { pub id: StorageId, pub tree: String, pub host: Arc<dyn AndroidDocuments>, pub writable: bool }
pub(crate) fn status(error: u32) -> Result<()> {
    match error {
        0 => Ok(()),
        1 => Err(CoreError::new(ErrorCode::PermissionDenied, "Android grant unavailable")),
        2 => Err(CoreError::new(ErrorCode::Unsupported, "Android provider capability unavailable")),
        4 => Err(CoreError::new(ErrorCode::LimitExceeded, "Android provider result exceeds limit")),
        5 => Err(CoreError::new(ErrorCode::NotFound, "Android document not found")),
        _ => Err(CoreError::new(ErrorCode::Io, "Android provider IO failed")),
    }
}
/// Host convention (E2.7): a file whose size the provider does not report
/// comes as `bytes == u64::MAX`. Listed for display, refused where the exact
/// size matters.
const UNKNOWN_SIZE: u64 = u64::MAX;
fn unknown_size(info: &HostInfo) -> bool { info.error == 0 && info.kind == 1 && info.bytes == UNKNOWN_SIZE }
fn metadata(info: HostInfo) -> Result<Metadata> {
    status(info.error)?;
    if unknown_size(&info) { return Err(CoreError::new(ErrorCode::Unsupported, "the provider does not report this file's size")); }
    Ok(Metadata { kind: match info.kind { 1 => EntryKind::File, 2 => EntryKind::Directory, _ => EntryKind::Other },
        bytes: info.bytes, modified_unix_ms: info.modified_ms })
}
pub(crate) fn exists() -> CoreError { CoreError::new(ErrorCode::AlreadyExists, "a document with this name already exists") }
fn internal(name: &str) -> bool { name.len() >= 10 && name.as_bytes()[..10].eq_ignore_ascii_case(b".paranoic-") }
/// Providers whose create/rename never overwrite (unique-name semantics).
pub(crate) fn trusted_writer(tree: &str) -> bool {
    tree.strip_prefix("content://").and_then(|rest| rest.split('/').next()) == Some("com.android.externalstorage.documents")
}
#[allow(dead_code)]
fn child(parent: &ResourcePath, name: &str) -> String { if parent.is_root() { name.to_owned() } else { format!("{}/{name}", parent.as_str()) } }

impl SafStorage {
    fn read_only(&self) -> Result<()> {
        if self.writable { Ok(()) } else { Err(CoreError::new(ErrorCode::PermissionDenied, "Android grant is read-only")) }
    }
    fn absent(&self, path: &ResourcePath) -> Result<()> {
        match metadata(self.host.metadata(self.tree.clone(), path.as_str().to_owned())) {
            Err(error) if error.code == ErrorCode::NotFound => Ok(()),
            Ok(_) => Err(exists()),
            Err(error) => Err(error),
        }
    }
    /// Create a document and confirm the provider kept exactly `name`;
    /// returns its URI. A renamed new FILE is ours and is deleted; a renamed
    /// new directory is left (directory deletion is recursive) and reported.
    fn create_exact(&self, parent: &ResourcePath, name: &str, directory: bool) -> Result<String> {
        let created = self.host.create(self.tree.clone(), parent.as_str().to_owned(), name.to_owned(), directory);
        status(created.error)?;
        if created.name != name {
            if directory { return Err(CoreError::new(ErrorCode::NeedsAttention, "name taken; provider created an extra empty folder")); }
            let _ = self.host.delete(self.tree.clone(), created.document, false);
            return Err(exists());
        }
        Ok(created.document)
    }
    fn move_between_folders(&self, source: &ResourcePath, from: &ResourcePath, old_name: &str, to: &ResourcePath,
        new_name: &str, target: &ResourcePath) -> Result<()> {
        self.absent(target)?;
        // The object keeps its name in the new folder; that name must be free there too.
        let interim = to.join(old_name)?;
        if old_name != new_name { self.absent(&interim)?; }
        let document = self.host.resolve(self.tree.clone(), source.as_str().to_owned());
        status(document.error)?;
        let (source_parent, target_parent) = (self.host.resolve(self.tree.clone(), from.as_str().to_owned()),
            self.host.resolve(self.tree.clone(), to.as_str().to_owned()));
        status(source_parent.error)?; status(target_parent.error)?;
        let moved = self.host.move_document(self.tree.clone(), document.document, source_parent.document.clone(), target_parent.document.clone());
        status(moved.error)?;
        if moved.name != old_name {
            return Err(CoreError::new(ErrorCode::NeedsAttention, "moved object got a provider-chosen name"));
        }
        if old_name == new_name { return Ok(()); }
        let undo = |document: String| {
            let back = self.host.move_document(self.tree.clone(), document, target_parent.document.clone(), source_parent.document.clone());
            if back.error == 0 && back.name == old_name { exists() }
            else { CoreError::new(ErrorCode::NeedsAttention, "object left in the target folder under its old name") }
        };
        let renamed = self.host.rename(self.tree.clone(), moved.document.clone(), new_name.to_owned());
        if renamed.error != 0 { return Err(undo(moved.document)); }
        if renamed.name != new_name {
            let back = self.host.rename(self.tree.clone(), renamed.document, old_name.to_owned());
            if back.error != 0 || back.name != old_name {
                return Err(CoreError::new(ErrorCode::NeedsAttention, "object left under a provider-chosen name"));
            }
            return Err(undo(back.document));
        }
        Ok(())
    }
    fn entries(&self, path: &ResourcePath, limit: usize, lenient: bool) -> Result<PartialListing> {
        let listing = self.host.list(self.tree.clone(), path.as_str().to_owned(), limit as u32);
        status(listing.error)?;
        if listing.entries.len() > limit { return Err(CoreError::new(ErrorCode::LimitExceeded, "directory limit")); }
        let mut counts = std::collections::HashMap::new();
        for e in &listing.entries { *counts.entry(e.name.clone()).or_insert(0usize) += 1; }
        let (mut entries, mut skipped, mut unknown) = (Vec::new(), 0u64, Vec::new());
        for e in listing.entries {
            if internal(&e.name) { continue; }
            let joined = path.join(&e.name);
            let usable = counts.get(&e.name) == Some(&1) && joined.is_ok();
            if !usable {
                if lenient { skipped += 1; continue; }
                return Err(CoreError::new(ErrorCode::Unsupported, "folder contains ambiguous or non-portable names"));
            }
            // Display: a file of unknown size is shown (size 0, marked); any
            // other document the provider cannot describe is skipped and counted.
            if lenient && unknown_size(&e.info) {
                let joined = joined?;
                unknown.push(joined.clone());
                entries.push(DirectoryEntry { location: Location { storage: self.id, path: joined },
                    metadata: Metadata { kind: EntryKind::File, bytes: 0, modified_unix_ms: e.info.modified_ms } });
                continue;
            }
            let meta = match metadata(e.info) { Ok(m) => m, Err(_) if lenient => { skipped += 1; continue; } Err(err) => return Err(err) };
            entries.push(DirectoryEntry { location: Location { storage: self.id, path: joined? }, metadata: meta });
        }
        entries.sort_by(|a, b| a.location.path.as_str().cmp(b.location.path.as_str()));
        Ok(PartialListing { entries, skipped, unknown_size: unknown })
    }
}

impl Storage for SafStorage {
    fn id(&self) -> StorageId { self.id }
    fn binding_key(&self) -> Result<String> { Ok(format!("android-saf:{}", self.tree)) }
    fn domain(&self) -> StorageDomain { StorageDomain::Files }
    /// Writable grants are only the platform's own storage provider (see
    /// `trusted_writer`): its rename never overwrites (it picks a unique name),
    /// so a same-folder rename with a name check is a move without replacement.
    /// No directory sync; creating a new vault tree is accepted anyway (E2.8,
    /// owner decision 2026-11-14).
    fn capabilities(&self) -> Capabilities {
        Capabilities { read: true, write: self.writable, delete: self.writable && self.host.deletion_supported(self.tree.clone()), atomic_move_no_replace: self.writable,
            directory_sync_waived: self.writable, ..Capabilities::default() }
    }
    fn metadata(&self, path: &ResourcePath) -> Result<Metadata> { metadata(self.host.metadata(self.tree.clone(), path.as_str().to_owned())) }
    fn list(&self, path: &ResourcePath, limit: usize) -> Result<Vec<DirectoryEntry>> { Ok(self.entries(path, limit, false)?.entries) }
    fn list_for_display(&self, path: &ResourcePath, limit: usize) -> Result<PartialListing> { self.entries(path, limit, true) }
    fn open_read(&self, path: &ResourcePath) -> Result<Box<dyn ReadHandle>> {
        let opened = self.host.open(self.tree.clone(), path.as_str().to_owned());
        status(opened.error)?;
        match metadata(opened.info) {
            Ok(info) if info.kind == EntryKind::File => Ok(Box::new(SafReader { host: Arc::clone(&self.host), id: opened.id, offset: 0 })),
            _ => { self.host.close(opened.id); Err(CoreError::new(ErrorCode::Unsupported, "not a readable document")) }
        }
    }
    fn stage(&self, target: &ResourcePath, job: JobId) -> Result<Box<dyn StagedWrite>> {
        self.read_only()?;
        let (parent, name) = target.parent_and_name()?;
        // No "target absent" check here: Core stages a replacement before it
        // moves the old file aside. Publication renames without replacing and
        // fails (cleaning up its own document) if the name is taken then.
        let temporary = format!(".paranoic-{}", job.0.simple());
        let document = self.create_exact(&parent, &temporary, false)?;
        let opened = self.host.open_write(self.tree.clone(), document.clone());
        if let Err(error) = status(opened.error) {
            let _ = self.host.delete(self.tree.clone(), document, false);
            return Err(error);
        }
        Ok(Box::new(SafStage { host: Arc::clone(&self.host), tree: self.tree.clone(), document,
            target: name.to_owned(), handle: Some(opened.id), finished: false }))
    }
    fn create_directory(&self, path: &ResourcePath) -> Result<()> {
        self.read_only()?;
        let (parent, name) = path.parent_and_name()?;
        self.absent(path)?;
        self.create_exact(&parent, name, true).map(|_| ())
    }
    /// Same folder: a rename. Another folder (E2.8): the host's moveDocument
    /// (keeps the name, never replaces), then a rename if the name changes.
    /// Two provider calls are not atomic: if the second fails, the first is
    /// undone; if that also fails the state is reported (NeedsAttention).
    fn move_no_replace(&self, source: &ResourcePath, target: &ResourcePath) -> Result<()> {
        self.read_only()?;
        let ((from, old_name), (to, new_name)) = (source.parent_and_name()?, target.parent_and_name()?);
        if from != to { return self.move_between_folders(source, &from, old_name, &to, new_name, target); }
        self.absent(target)?;
        let resolved = self.host.resolve(self.tree.clone(), source.as_str().to_owned());
        status(resolved.error)?;
        let renamed = self.host.rename(self.tree.clone(), resolved.document, new_name.to_owned());
        status(renamed.error)?;
        if renamed.name != new_name {
            // The name was taken meanwhile: the provider chose another. Put the
            // object back under its own (now free) name and report the conflict.
            let back = self.host.rename(self.tree.clone(), renamed.document, old_name.to_owned());
            if back.error != 0 || back.name != old_name {
                return Err(CoreError::new(ErrorCode::NeedsAttention, "object left under a provider-chosen name"));
            }
            return Err(exists());
        }
        Ok(())
    }
    // Cryptomator renames that change the shortening class (android_mapped.rs).
    fn rename_mapped(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, old: &[u8], new: &[u8]) -> Result<()> {
        self.read_only()?; self.saf_rename_mapped(source, target, job, old, new)
    }
    fn inspect_mapped_rename(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, old: &[u8], new: &[u8],
        checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<MappedRenameObservation> {
        self.saf_inspect_mapped(source, target, job, old, new, checkpoint)
    }
    #[allow(clippy::too_many_arguments)]
    fn recover_mapped(&self, source: &ResourcePath, target: &ResourcePath, original: JobId, recovery: JobId,
        old: &[u8], new: &[u8], action: MappedRecoveryAction) -> Result<()> {
        self.read_only()?; self.saf_recover_mapped(source, target, original, recovery, old, new, action)
    }
    fn reshape_directory(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, layout: &DirectoryReshape) -> Result<()> {
        self.read_only()?; self.saf_reshape(source, target, job, layout)
    }
    fn inspect_reshape_directory(&self, source: &ResourcePath, target: &ResourcePath, job: JobId, layout: &DirectoryReshape,
        checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<MappedRenameObservation> {
        self.saf_inspect_reshape(source, target, job, layout, checkpoint)
    }
    fn recover_reshape_directory(&self, source: &ResourcePath, target: &ResourcePath, original: JobId, recovery: JobId,
        layout: &DirectoryReshape, action: MappedRecoveryAction) -> Result<()> {
        self.read_only()?; self.saf_recover_reshape(source, target, original, recovery, layout, action)
    }
    /// A NEW vault tree (E2.8): its folder is created first and must not
    /// exist; files go in one by one through the normal staged write. Nothing
    /// existing is replaced. Core reads the tree back afterwards; a failure
    /// part-way leaves a partial new folder, reported as NeedsAttention.
    fn publish_tree(&self, target: &ResourcePath, job: JobId, files: &[PreparedTreeFile],
        checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        self.read_only()?;
        checkpoint()?;
        self.create_directory(target)?;
        let mut made = std::collections::BTreeSet::new();
        let result = (|| -> Result<()> {
            for file in files {
                checkpoint()?;
                let path = ResourcePath::parse(format!("{}/{}", target.as_str(), file.path.as_str()))?;
                let (parent, _) = path.parent_and_name()?;
                let mut current = target.clone();
                if let Some(rest) = parent.as_str().strip_prefix(target.as_str()) {
                    for part in rest.split('/').filter(|p| !p.is_empty()) {
                        current = current.join(part)?;
                        if made.insert(current.as_str().to_owned()) { self.create_directory(&current)?; }
                    }
                }
                let mut staged = self.stage(&path, job)?;
                if let Err(error) = staged.write_all(&file.bytes) { let _ = staged.abort(); return Err(CoreError::io(error)); }
                staged.publish()?;
            }
            Ok(())
        })();
        result.map_err(|error| if error.code == ErrorCode::Cancelled || error.code == ErrorCode::NeedsAttention { error }
            else { CoreError::new(ErrorCode::NeedsAttention, "new vault folder only partly written; remove it and try again") })
    }
    fn delete_file(&self, path: &ResourcePath) -> Result<()> {
        self.read_only()?;
        if !self.host.deletion_supported(self.tree.clone()) {
            return Err(CoreError::new(ErrorCode::Unsupported, "atomic SAF deletion unavailable"));
        }
        if path.is_root() { return Err(CoreError::new(ErrorCode::InvalidInput, "root is never deleted")); }
        let resolved = self.host.resolve(self.tree.clone(), path.as_str().to_owned());
        status(resolved.error)?;
        if self.metadata(path)?.kind != EntryKind::File { return Err(CoreError::new(ErrorCode::Unsupported, "not a file")); }
        status(self.host.delete(self.tree.clone(), resolved.document, false))
    }
    /// Only an EMPTY folder. The host must use atomic rmdir: a listing cannot
    /// prove it is still empty when a provider performs recursive deletion.
    fn delete_empty_directory(&self, path: &ResourcePath) -> Result<()> {
        self.read_only()?;
        if !self.host.deletion_supported(self.tree.clone()) {
            return Err(CoreError::new(ErrorCode::Unsupported, "atomic SAF deletion unavailable"));
        }
        if path.is_root() { return Err(CoreError::new(ErrorCode::InvalidInput, "root is never deleted")); }
        if self.metadata(path)?.kind != EntryKind::Directory { return Err(CoreError::new(ErrorCode::Unsupported, "not a folder")); }
        let resolved = self.host.resolve(self.tree.clone(), path.as_str().to_owned());
        status(resolved.error)?;
        status(self.host.delete(self.tree.clone(), resolved.document, true))
    }
    /// A new folder `target` with a bounded sidecar and one streamed file
    /// (Cryptomator long names). Built in a hidden folder and renamed into
    /// place without replacing; abort removes only that hidden folder.
    fn stage_bundle(&self, target: &ResourcePath, job: JobId, content_name: &str, sidecar_name: &str,
        sidecar: &[u8]) -> Result<Box<dyn StagedWrite>> {
        self.read_only()?;
        let (parent, name) = target.parent_and_name()?;
        self.absent(target)?;
        let hidden = format!(".paranoic-b{}", job.0.simple());
        let folder = self.host.create(self.tree.clone(), parent.as_str().to_owned(), hidden.clone(), true);
        status(folder.error)?;
        if folder.name != hidden {
            return Err(CoreError::new(ErrorCode::NeedsAttention, "staging folder name taken; provider created an extra empty folder"));
        }
        let inside = if parent.is_root() { hidden.clone() } else { format!("{}/{hidden}", parent.as_str()) };
        let built = (|| -> Result<u64> {
            let side = self.host.create(self.tree.clone(), inside.clone(), sidecar_name.to_owned(), false);
            status(side.error)?;
            if side.name != sidecar_name { return Err(exists()); }
            let opened = self.host.open_write(self.tree.clone(), side.document);
            status(opened.error)?;
            if let Err(error) = status(self.host.write(opened.id, sidecar.to_vec())) { let _ = self.host.finish_write(opened.id); return Err(error); }
            status(self.host.finish_write(opened.id))?;
            let content = self.host.create(self.tree.clone(), inside.clone(), content_name.to_owned(), false);
            status(content.error)?;
            if content.name != content_name { return Err(exists()); }
            let opened = self.host.open_write(self.tree.clone(), content.document);
            status(opened.error)?;
            Ok(opened.id)
        })();
        match built {
            Ok(handle) => Ok(Box::new(SafBundle { host: Arc::clone(&self.host), tree: self.tree.clone(), folder: folder.document,
                target: name.to_owned(), handle: Some(handle), finished: false })),
            Err(error) => {
                match status(self.host.delete(self.tree.clone(), folder.document, true)) {
                    Ok(()) => Err(error),
                    Err(_) => Err(CoreError::new(ErrorCode::NeedsAttention, "staging bundle preserved; cleanup required")),
                }
            }
        }
    }
}

struct SafStage { host: Arc<dyn AndroidDocuments>, tree: String, document: String, target: String,
    handle: Option<u64>, finished: bool }
impl SafStage {
    fn close(&mut self) -> Result<()> {
        match self.handle.take() { Some(handle) => status(self.host.finish_write(handle)), None => Ok(()) }
    }
    fn discard(&mut self) -> Result<()> {
        let closed = self.close();
        let deleted = status(self.host.delete(self.tree.clone(), self.document.clone(), false));
        self.finished = true;
        closed.and(deleted).map_err(|_| CoreError::new(ErrorCode::NeedsAttention, "temporary document cleanup failed"))
    }
}
impl Write for SafStage {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let handle = self.handle.ok_or_else(|| io::Error::other("closed document"))?;
        let part = &data[..data.len().min(1 << 20)];
        status(self.host.write(handle, part.to_vec())).map_err(|_| io::Error::other("Android write failed"))?;
        Ok(part.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
impl StagedWrite for SafStage {
    fn publish(mut self: Box<Self>) -> Result<()> {
        if let Err(error) = self.close() { self.discard()?; return Err(error); }
        let renamed = self.host.rename(self.tree.clone(), self.document.clone(), self.target.clone());
        if let Err(error) = status(renamed.error) { self.discard()?; return Err(error); }
        self.finished = true;
        if renamed.name != self.target {
            // Name was taken: remove our own renamed document (by identity), never the other one.
            return match status(self.host.delete(self.tree.clone(), renamed.document, false)) {
                Ok(()) => Err(exists()),
                Err(_) => Err(CoreError::new(ErrorCode::NeedsAttention, "conflicting copy left under a provider-chosen name")),
            };
        }
        Ok(())
    }
    fn abort(mut self: Box<Self>) -> Result<()> { self.discard() }
}
impl Drop for SafStage { fn drop(&mut self) { if !self.finished { let _ = self.discard(); } } }

/// A staged bundle folder (see `stage_bundle`): writes go to its content file.
struct SafBundle { host: Arc<dyn AndroidDocuments>, tree: String, folder: String, target: String, handle: Option<u64>, finished: bool }
impl SafBundle {
    fn discard(&mut self) -> Result<()> {
        if let Some(handle) = self.handle.take() { let _ = self.host.finish_write(handle); }
        if self.finished { return Ok(()); }
        self.finished = true;
        // A staged folder can acquire somebody else's files too. Never recurse;
        // leave a nonempty bundle as evidence and report that cleanup is needed.
        status(self.host.delete(self.tree.clone(), self.folder.clone(), true))
            .map_err(|_| CoreError::new(ErrorCode::NeedsAttention, "staging bundle preserved; cleanup required"))
    }
}
impl Write for SafBundle {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let handle = self.handle.ok_or_else(|| io::Error::other("closed"))?;
        status(self.host.write(handle, data.to_vec())).map_err(|_| io::Error::other("Android write failed"))?;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
impl StagedWrite for SafBundle {
    fn publish(mut self: Box<Self>) -> Result<()> {
        if let Some(handle) = self.handle.take() {
            if let Err(error) = status(self.host.finish_write(handle)) { self.discard()?; return Err(error); }
        }
        let renamed = self.host.rename(self.tree.clone(), self.folder.clone(), self.target.clone());
        if let Err(error) = status(renamed.error) { self.discard()?; return Err(error); }
        self.finished = true;
        if renamed.name != self.target {
            // An occupied bundle is preserved; only an actually empty folder can go.
            return match status(self.host.delete(self.tree.clone(), renamed.document, true)) {
                Ok(()) => Err(exists()),
                Err(_) => Err(CoreError::new(ErrorCode::NeedsAttention, "conflicting bundle left under a provider-chosen name")),
            };
        }
        Ok(())
    }
    fn abort(mut self: Box<Self>) -> Result<()> { self.discard() }
}
impl Drop for SafBundle { fn drop(&mut self) { if !self.finished { let _ = self.discard(); } } }

struct SafReader { host: Arc<dyn AndroidDocuments>, id: u64, offset: u64 }
impl Read for SafReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = output.len().min(1024 * 1024) as u32;
        let read = self.host.read(self.id, self.offset, count);
        // Code 2: the stream cannot jump (pipe); archives then use a private copy.
        status(read.error).map_err(|e| if e.code == ErrorCode::Unsupported { io::Error::new(io::ErrorKind::Unsupported, "no random access") }
            else { io::Error::other("Android read failed") })?;
        if read.data.len() > count as usize { return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid host buffer")); }
        output[..read.data.len()].copy_from_slice(&read.data);
        self.offset += read.data.len() as u64;
        Ok(read.data.len())
    }
}
impl ReadHandle for SafReader {
    fn metadata(&self) -> Result<Metadata> { metadata(self.host.file_metadata(self.id)) }
    /// Streams from pipe-backed providers (e.g. cloud documents) reject a
    /// non-sequential offset in the host (Unsupported) on the next read.
    fn seek_to(&mut self, offset: u64) -> Result<()> { self.offset = offset; Ok(()) }
}
impl Drop for SafReader { fn drop(&mut self) { self.host.close(self.id); } }
