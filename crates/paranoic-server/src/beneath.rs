//! Linux/Android file system for the server with no check-then-use window
//! (register У-23). Every path is resolved component by component from an
//! open descriptor of the root with `openat(O_NOFOLLOW)`, and every operation
//! (open, list, mkdir, rename, copy, trash) is an `*at` call on the resolved
//! parent descriptor. A symlink anywhere — present from the start or swapped
//! in concurrently by a local user — makes the kernel refuse the step, so no
//! request can read, write, list or move anything outside the root.
//!
//! Symlinks are invisible (not listed, NotFound on access). The trash folder
//! is invisible and inaccessible. Renames and copies never replace.
use std::{ffi::{OsStr, OsString}, fs::File, io::{Read, Seek, SeekFrom, Write}, os::{fd::OwnedFd, unix::ffi::OsStrExt},
    path::{Component, Path}, time::{SystemTime, UNIX_EPOCH}};
use dav_server::{davpath::DavPath, fs::*};
use futures_util::FutureExt;
use rustix::fs::{self as rfs, Mode, OFlags, RenameFlags};
use std::os::unix::fs::MetadataExt;

const TRASH: &str = super::TRASH;

#[derive(Clone)]
pub struct BeneathFs { root: std::sync::Arc<OwnedFd>, quota: dav_server::localfs::LocalFs }

fn dir_flags() -> OFlags { OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC }

/// Map errors: a symlink in the way (ELOOP) or a non-directory component is "not found".
fn fs_error(error: std::io::Error) -> FsError {
    match error.raw_os_error() {
        Some(code) if code == rustix::io::Errno::LOOP.raw_os_error() || code == rustix::io::Errno::NOTDIR.raw_os_error() => FsError::NotFound,
        _ => FsError::from(error),
    }
}
fn io(e: rustix::io::Errno) -> std::io::Error { std::io::Error::from(e) }

/// Relative path components; anything but plain names is refused.
fn parts(path: &DavPath) -> Result<Vec<OsString>, FsError> {
    let rel = path.as_rel_ospath();
    let mut out = Vec::new();
    for c in Path::new(rel).components() {
        match c { Component::Normal(n) => out.push(n.to_owned()), _ => return Err(FsError::Forbidden) }
    }
    if out.first().is_some_and(|n| n.as_bytes().eq_ignore_ascii_case(TRASH.as_bytes())) { return Err(FsError::NotFound); }
    Ok(out)
}

impl BeneathFs {
    pub fn open(root: &Path) -> std::io::Result<Self> {
        let fd = rfs::open(root, dir_flags(), Mode::empty()).map_err(io)?;
        Ok(Self { root: std::sync::Arc::new(fd), quota: *dav_server::localfs::LocalFs::new(root, false, false, false) })
    }
    /// Descriptor of the directory `names` (relative to the root), no symlinks followed.
    fn dir(&self, names: &[OsString]) -> std::io::Result<OwnedFd> {
        let mut current = rfs::openat(&*self.root, ".", dir_flags(), Mode::empty()).map_err(io)?;
        for name in names { current = rfs::openat(&current, name.as_os_str(), dir_flags(), Mode::empty()).map_err(io)?; }
        Ok(current)
    }
    /// Parent descriptor and final name. The root itself has no parent.
    fn parent(&self, path: &DavPath) -> Result<(OwnedFd, OsString), FsError> {
        let mut names = parts(path)?;
        let name = names.pop().ok_or(FsError::Forbidden)?;
        Ok((self.dir(&names).map_err(fs_error)?, name))
    }
    /// Metadata without following a final symlink; a symlink is "not found".
    fn stat(&self, path: &DavPath) -> Result<std::fs::Metadata, FsError> {
        let names = parts(path)?;
        let fd = match names.split_last() {
            None => self.dir(&[]).map_err(fs_error)?,
            Some((name, parents)) => {
                let parent = self.dir(parents).map_err(fs_error)?;
                rfs::openat(&parent, name.as_os_str(), OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())
                    .map_err(|e| fs_error(io(e)))?
            }
        };
        let meta = File::from(fd).metadata().map_err(fs_error)?;
        if meta.file_type().is_symlink() { return Err(FsError::NotFound); }
        Ok(meta)
    }
    async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, FsError> + Send + 'static) -> FsResult<T> {
        tokio::task::spawn_blocking(f).await.map_err(|_| FsError::GeneralFailure)?
    }
}

#[derive(Clone, Debug)]
struct Meta(std::fs::Metadata);
impl DavMetaData for Meta {
    fn len(&self) -> u64 { self.0.len() }
    fn modified(&self) -> FsResult<SystemTime> { self.0.modified().map_err(FsError::from) }
    fn is_dir(&self) -> bool { self.0.is_dir() }
    fn is_file(&self) -> bool { self.0.is_file() }
    /// Same form as dav-server LocalFs (inode, length, mtime): clients keep their ETags.
    fn etag(&self) -> Option<String> {
        let t = self.0.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        let t = t.as_secs() * 1_000_000 + u64::from(t.subsec_nanos()) / 1000;
        Some(if self.0.is_file() { format!("{:x}-{:x}-{:x}", self.0.ino(), self.0.len(), t) } else { format!("{:x}-{:x}", self.0.ino(), t) })
    }
}

struct Entry { name: Vec<u8>, meta: Meta }
impl DavDirEntry for Entry {
    fn name(&self) -> Vec<u8> { self.name.clone() }
    fn metadata(&self) -> FsFuture<'_, Box<dyn DavMetaData>> { let m = self.meta.clone(); async move { Ok(Box::new(m) as Box<dyn DavMetaData>) }.boxed() }
}

#[derive(Debug)]
struct BeneathFile(Option<File>);
impl BeneathFile {
    async fn with<T: Send + 'static>(&mut self, f: impl FnOnce(&mut File) -> std::io::Result<T> + Send + 'static) -> FsResult<T> {
        let mut file = self.0.take().ok_or(FsError::GeneralFailure)?;
        let (file, result) = tokio::task::spawn_blocking(move || { let r = f(&mut file); (file, r) }).await.map_err(|_| FsError::GeneralFailure)?;
        self.0 = Some(file);
        result.map_err(FsError::from)
    }
}
impl DavFile for BeneathFile {
    fn metadata(&mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        async move { let m = self.with(|f| f.metadata()).await?; Ok(Box::new(Meta(m)) as Box<dyn DavMetaData>) }.boxed()
    }
    fn write_buf(&mut self, mut buf: Box<dyn bytes::Buf + Send>) -> FsFuture<'_, ()> {
        async move {
            let data = buf.copy_to_bytes(buf.remaining());
            self.with(move |f| f.write_all(&data)).await
        }.boxed()
    }
    fn write_bytes(&mut self, buf: bytes::Bytes) -> FsFuture<'_, ()> { async move { self.with(move |f| f.write_all(&buf)).await }.boxed() }
    fn read_bytes(&mut self, count: usize) -> FsFuture<'_, bytes::Bytes> {
        async move {
            let count = count.min(1 << 20);
            self.with(move |f| {
                let mut out = vec![0u8; count];
                let n = f.read(&mut out)?;
                out.truncate(n);
                Ok(bytes::Bytes::from(out))
            }).await
        }.boxed()
    }
    fn seek(&mut self, pos: SeekFrom) -> FsFuture<'_, u64> { async move { self.with(move |f| f.seek(pos)).await }.boxed() }
    fn flush(&mut self) -> FsFuture<'_, ()> { async move { self.with(|f| f.sync_all()).await }.boxed() }
}

fn no_replace_rename(from: &OwnedFd, from_name: &OsStr, to: &OwnedFd, to_name: &OsStr) -> Result<(), FsError> {
    rfs::renameat_with(from, from_name, to, to_name, RenameFlags::NOREPLACE).map_err(|e| fs_error(io(e)))
}

impl DavFileSystem for BeneathFs {
    fn open<'a>(&'a self, path: &'a DavPath, options: OpenOptions) -> FsFuture<'a, Box<dyn DavFile>> {
        let this = self.clone();
        let path = path.clone();
        Self::blocking(move || {
            let (parent, name) = this.parent(&path)?;
            let mut flags = OFlags::NOFOLLOW | OFlags::CLOEXEC;
            flags |= match (options.read, options.write || options.append) {
                (true, true) => OFlags::RDWR, (false, true) => OFlags::WRONLY, _ => OFlags::RDONLY,
            };
            if options.append { flags |= OFlags::APPEND; }
            if options.truncate { flags |= OFlags::TRUNC; }
            if options.create_new { flags |= OFlags::CREATE | OFlags::EXCL; } else if options.create { flags |= OFlags::CREATE; }
            let fd = rfs::openat(&parent, name.as_os_str(), flags, Mode::from_raw_mode(0o600)).map_err(|e| fs_error(io(e)))?;
            let file = File::from(fd);
            if !file.metadata().map_err(FsError::from)?.is_file() { return Err(FsError::Forbidden); }
            Ok(Box::new(BeneathFile(Some(file))) as Box<dyn DavFile>)
        }).boxed()
    }
    fn read_dir<'a>(&'a self, path: &'a DavPath, _meta: ReadDirMeta) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        let this = self.clone();
        let path = path.clone();
        async move {
            let entries = Self::blocking(move || {
                let names = parts(&path)?;
                let top = names.is_empty();
                let dir = this.dir(&names).map_err(fs_error)?;
                let readable = rfs::openat(&dir, ".", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())
                    .map_err(|e| fs_error(io(e)))?;
                let mut out = Vec::new();
                for entry in rfs::Dir::read_from(&readable).map_err(|e| fs_error(io(e)))? {
                    let entry = entry.map_err(|e| fs_error(io(e)))?;
                    let name = entry.file_name().to_bytes().to_vec();
                    if name == b"." || name == b".." || top && name.eq_ignore_ascii_case(TRASH.as_bytes()) { continue; }
                    let Ok(fd) = rfs::openat(&dir, OsStr::from_bytes(&name), OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) else { continue };
                    let Ok(meta) = File::from(fd).metadata() else { continue };
                    if meta.file_type().is_symlink() || !(meta.is_file() || meta.is_dir()) { continue; }
                    out.push(Box::new(Entry { name, meta: Meta(meta) }) as Box<dyn DavDirEntry>);
                }
                Ok(out)
            }).await?;
            Ok(Box::pin(futures_util::stream::iter(entries.into_iter().map(Ok))) as FsStream<Box<dyn DavDirEntry>>)
        }.boxed()
    }
    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        let this = self.clone();
        let path = path.clone();
        Self::blocking(move || Ok(Box::new(Meta(this.stat(&path)?)) as Box<dyn DavMetaData>)).boxed()
    }
    fn symlink_metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> { DavFileSystem::metadata(self, path) }
    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        let this = self.clone();
        let path = path.clone();
        Self::blocking(move || {
            let (parent, name) = this.parent(&path)?;
            rfs::mkdirat(&parent, name.as_os_str(), Mode::from_raw_mode(0o700)).map_err(|e| fs_error(io(e)))
        }).boxed()
    }
    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> { self.to_trash(path) }
    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> { self.to_trash(path) }
    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        let this = self.clone();
        let (from, to) = (from.clone(), to.clone());
        Self::blocking(move || {
            let (a, an) = this.parent(&from)?;
            let (b, bn) = this.parent(&to)?;
            no_replace_rename(&a, &an, &b, &bn)
        }).boxed()
    }
    fn copy<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        let this = self.clone();
        let (from, to) = (from.clone(), to.clone());
        Self::blocking(move || {
            let (a, an) = this.parent(&from)?;
            let (b, bn) = this.parent(&to)?;
            let source = File::from(rfs::openat(&a, an.as_os_str(), OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())
                .map_err(|e| fs_error(io(e)))?);
            if !source.metadata().map_err(FsError::from)?.is_file() { return Err(FsError::Forbidden); }
            let target = File::from(rfs::openat(&b, bn.as_os_str(), OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600)).map_err(|e| fs_error(io(e)))?);
            let (mut source, mut target) = (source, target);
            std::io::copy(&mut source, &mut target).map_err(FsError::from)?;
            target.sync_all().map_err(FsError::from)
        }).boxed()
    }
    fn get_quota(&self) -> FsFuture<'_, (u64, Option<u64>)> { DavFileSystem::get_quota(&self.quota) }
}

impl BeneathFs {
    /// DELETE: move into a fresh bucket of the hidden trash, never erase.
    fn to_trash<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        let this = self.clone();
        let path = path.clone();
        Self::blocking(move || {
            let (parent, name) = this.parent(&path)?;
            let trash = rfs::openat(&*this.root, TRASH, dir_flags(), Mode::empty()).map_err(|e| fs_error(io(e)))?;
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
            let bucket = format!("{stamp}-{}", uuid::Uuid::new_v4().simple());
            rfs::mkdirat(&trash, bucket.as_str(), Mode::from_raw_mode(0o700)).map_err(|e| fs_error(io(e)))?;
            let bucket = rfs::openat(&trash, bucket.as_str(), dir_flags(), Mode::empty()).map_err(|e| fs_error(io(e)))?;
            no_replace_rename(&parent, &name, &bucket, &name)
        }).boxed()
    }
}
