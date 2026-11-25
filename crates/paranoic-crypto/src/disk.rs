use std::{io::{Read, Write}, path::Path};

use cap_std::{ambient_authority, fs::{Dir, File, OpenOptions}};

use crate::{random, CryptoError, Result, StreamSink};

/// Opaque object names prevent traversal and accidental reuse of user paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ObjectId([u8; 16]);

impl ObjectId {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0; 16];
        random(&mut bytes)?;
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self { Self(bytes) }
    pub fn as_bytes(&self) -> &[u8; 16] { &self.0 }
    /// Lowercase hex, as the object's file name.
    pub fn hex(&self) -> String { self.name() }
    /// Parse 32 lowercase hex digits (the form `hex` produces).
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 32 || !text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return None; }
        let mut bytes = [0u8; 16];
        for (i, byte) in bytes.iter_mut().enumerate() { *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?; }
        Some(Self(bytes))
    }

    fn name(self) -> String {
        let mut name = String::with_capacity(32);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in self.0 {
            name.push(HEX[(byte >> 4) as usize] as char);
            name.push(HEX[(byte & 15) as usize] as char);
        }
        name
    }
}

/// Host-owned private application directory, never a Files/Cloud grant.
/// Host must disable system backup for sensitive data and secure its ancestors.
/// This API does not protect against another process running as the same user.
///
/// One owner at a time: open() takes an exclusive lock (`.private.lock`, held
/// for the lifetime of this value) and then removes `.pending-*` leftovers of
/// writes interrupted by a crash or kill, which may contain plaintext. Removal
/// is an unlink, not physical erasure of flash blocks.
///
/// Windows: the directory must be owned by the current user and accessible
/// only to that user, SYSTEM and Administrators (`paranoic_windows::
/// ensure_owner_only` sets this); it is verified on open. NTFS has no
/// directory fsync: a power loss right after commit can lose the new object,
/// never expose a partial one.
pub struct PrivateDirectory { directory: Dir, _lock: std::fs::File }

impl PrivateDirectory {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        #[cfg(windows)]
        if !matches!(paranoic_windows::is_owner_only(path.as_ref()), Ok(true)) { return Err(CryptoError::Permissions); }
        let directory = Dir::open_ambient_dir(path, ambient_authority()).map_err(|_| CryptoError::Io)?;
        #[cfg(unix)]
        {
            use cap_std::fs::{MetadataExt, PermissionsExt};
            let metadata = directory.dir_metadata().map_err(|_| CryptoError::Io)?;
            if metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(CryptoError::Permissions);
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = directory.open_with(".private.lock", &options).map_err(|_| CryptoError::Io)?.into_std();
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| CryptoError::Busy)?;
        let private = Self { directory, _lock: lock };
        private.remove_interrupted()?;
        Ok(private)
    }

    /// With the exclusive lock held no writer of this directory is alive, so
    /// every `.pending-*` regular file is an abandoned temporary object.
    fn remove_interrupted(&self) -> Result<()> {
        let mut removed = false;
        for entry in self.directory.entries().map_err(|_| CryptoError::Io)? {
            let entry = entry.map_err(|_| CryptoError::Io)?;
            let Ok(name) = entry.file_name().into_string() else { continue };
            if !name.starts_with(".pending-") { continue; }
            let metadata = self.directory.symlink_metadata(&name).map_err(|_| CryptoError::Io)?;
            if metadata.is_file() {
                self.directory.remove_file(&name).map_err(|_| CryptoError::CleanupFailed)?;
                removed = true;
            }
        }
        if removed { sync(&self.directory).map_err(|_| CryptoError::CleanupFailed)?; }
        Ok(())
    }

    pub fn stage(&self, destination: ObjectId) -> Result<Box<dyn StreamSink>> {
        let temporary = format!(".pending-{}", ObjectId::generate()?.name());
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let directory = self.directory.try_clone().map_err(|_| CryptoError::Io)?;
        let file = directory.open_with(&temporary, &options).map_err(|_| CryptoError::Io)?;
        Ok(Box::new(DiskSink { directory, temporary, destination: destination.name(), file: Some(file), cleaned: false, replace: false }))
    }

    /// Like `stage`, but commit atomically replaces an existing object (rename
    /// over it after the data is flushed). For small mutable records owned by
    /// one writer (the directory lock guarantees that); readers see the old or
    /// the new object, never a mix.
    pub fn stage_replace(&self, destination: ObjectId) -> Result<Box<dyn StreamSink>> {
        let temporary = format!(".pending-{}", ObjectId::generate()?.name());
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let directory = self.directory.try_clone().map_err(|_| CryptoError::Io)?;
        let file = directory.open_with(&temporary, &options).map_err(|_| CryptoError::Io)?;
        Ok(Box::new(DiskSink { directory, temporary, destination: destination.name(), file: Some(file), cleaned: false, replace: true }))
    }

    /// Whether an object exists (regular file only).
    pub fn contains(&self, object: ObjectId) -> Result<bool> {
        match self.directory.symlink_metadata(object.name()) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(CryptoError::Io),
        }
    }

    /// Reading is explicit; the private directory is never registered in the
    /// general file API. Caller controls the lifetime of plaintext it reads.
    /// Stored length of a regular-file object (no symlinks), without reading it.
    pub fn object_len(&self, object: ObjectId) -> Result<u64> {
        let metadata = self.directory.symlink_metadata(object.name()).map_err(|_| CryptoError::Io)?;
        if !metadata.is_file() { return Err(CryptoError::InvalidInput); }
        Ok(metadata.len())
    }
    pub fn open_object(&self, object: ObjectId) -> Result<impl Read> {
        let name = object.name();
        let metadata = self.directory.symlink_metadata(&name).map_err(|_| CryptoError::Io)?;
        if !metadata.is_file() { return Err(CryptoError::InvalidInput); }
        self.directory.open(name).map_err(|_| CryptoError::Io)
    }

    /// Remove every object (32-hex-digit regular file) in this directory.
    /// For directories dedicated to one ephemeral store, e.g. a session cache
    /// whose previous key is gone. Returns the number removed.
    pub fn clear_objects(&self) -> Result<usize> {
        let mut removed = 0;
        for entry in self.directory.entries().map_err(|_| CryptoError::Io)? {
            let entry = entry.map_err(|_| CryptoError::Io)?;
            let Ok(name) = entry.file_name().into_string() else { continue };
            if name.len() != 32 || !name.bytes().all(|b| b.is_ascii_hexdigit()) { continue; }
            if self.directory.symlink_metadata(&name).map_err(|_| CryptoError::Io)?.is_file() {
                self.directory.remove_file(&name).map_err(|_| CryptoError::CleanupFailed)?;
                removed += 1;
            }
        }
        if removed > 0 { sync(&self.directory).map_err(|_| CryptoError::CleanupFailed)?; }
        Ok(removed)
    }

    /// Every object (32-hex-digit regular file) in this directory.
    pub fn list_objects(&self) -> Result<Vec<ObjectId>> {
        let mut out = Vec::new();
        for entry in self.directory.entries().map_err(|_| CryptoError::Io)? {
            let entry = entry.map_err(|_| CryptoError::Io)?;
            let Ok(name) = entry.file_name().into_string() else { continue };
            let Some(id) = ObjectId::parse(&name) else { continue };
            if self.directory.symlink_metadata(&name).map_err(|_| CryptoError::Io)?.is_file() { out.push(id); }
        }
        Ok(out)
    }

    pub fn remove_object(&self, object: ObjectId) -> Result<()> {
        self.directory.remove_file(object.name()).map_err(|_| CryptoError::Io)?;
        sync(&self.directory).map_err(|_| CryptoError::PublicationUncertain)
    }
}

struct DiskSink {
    directory: Dir,
    temporary: String,
    destination: String,
    file: Option<File>,
    cleaned: bool,
    replace: bool,
}

impl DiskSink {
    fn cleanup(&mut self) -> Result<()> {
        self.file.take();
        self.directory.remove_file(&self.temporary).map_err(|_| CryptoError::CleanupFailed)?;
        self.cleaned = true;
        sync(&self.directory).map_err(|_| CryptoError::CleanupFailed)
    }
}

impl Write for DiskSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.file.as_mut().ok_or_else(closed)?.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.as_mut().ok_or_else(closed)?.flush()
    }
}

impl StreamSink for DiskSink {
    fn commit(mut self: Box<Self>) -> Result<()> {
        let mut file = self.file.take().ok_or(CryptoError::InvalidInput)?;
        file.flush().map_err(|_| CryptoError::Io)?;
        file.sync_all().map_err(|_| CryptoError::Io)?;
        drop(file);
        if self.replace {
            if self.directory.rename(&self.temporary, &self.directory, &self.destination).is_err() {
                self.cleanup()?;
                return Err(CryptoError::Io);
            }
            self.cleaned = true;
            return sync(&self.directory).map_err(|_| CryptoError::PublicationUncertain);
        }
        // Create-new without replacing. Linux/Android: renameat2(RENAME_NOREPLACE)
        // (Android forbids hard links in app data); link() only as a fallback
        // for filesystems without RENAME_NOREPLACE.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        match rustix::fs::renameat_with(&self.directory, self.temporary.as_str(), &self.directory, self.destination.as_str(),
            rustix::fs::RenameFlags::NOREPLACE) {
            Ok(()) => {
                self.cleaned = true;
                return sync(&self.directory).map_err(|_| CryptoError::PublicationUncertain);
            }
            Err(e) if e == rustix::io::Errno::INVAL || e == rustix::io::Errno::NOSYS => {}
            Err(e) => {
                self.cleanup()?;
                return Err(if e == rustix::io::Errno::EXIST { CryptoError::AlreadyExists } else { CryptoError::Io });
            }
        }
        if let Err(error) = self.directory.hard_link(&self.temporary, &self.directory, &self.destination) {
            self.cleanup()?;
            return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
                CryptoError::AlreadyExists
            } else { CryptoError::Io });
        }
        sync(&self.directory).map_err(|_| CryptoError::PublicationUncertain)?;
        self.cleanup().map_err(|_| CryptoError::PublicationUncertain)
    }

    fn abort(mut self: Box<Self>) -> Result<()> { self.cleanup() }
}

impl Drop for DiskSink {
    fn drop(&mut self) {
        self.file.take();
        if !self.cleaned { let _ = self.directory.remove_file(&self.temporary); }
    }
}

fn sync(directory: &Dir) -> Result<()> {
    #[cfg(unix)]
    {
        // Reopen O_PATH handles for fsync without resolving a public pathname.
        let fd = rustix::fs::openat(directory, ".",
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty()).map_err(|_| CryptoError::Io)?;
        std::fs::File::from(fd).sync_all().map_err(|_| CryptoError::Io)
    }
    #[cfg(not(unix))]
    {
        // No portable directory flush (see PrivateDirectory docs); file data
        // itself is flushed with sync_all before publication.
        let _ = directory;
        Ok(())
    }
}

fn closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "private output closed")
}

