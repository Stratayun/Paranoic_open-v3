//! Windows staging. The staging folder is made owner-only (protected ACL:
//! user, SYSTEM, Administrators) and verified before any data is written —
//! the counterpart of the 0700 check on Unix — so other local users with
//! write access to the parent cannot swap the payload.
//! A file is published by a hard link (atomic create-new), never exists+rename.
//! Directory replacement and power-loss directory durability stay unsupported.
use super::*;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::OpenOptionsExt;
use std::os::windows::io::AsHandle;
use windows_sys::Win32::Storage::FileSystem::{DELETE, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    FILE_GENERIC_READ, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE};

pub(super) fn move_no_replace(source_parent: &Dir, source_name: &str, target_parent: &Dir, target_name: &str) -> Result<()> {
    let mut options = OpenOptions::new();
    options.access_mode(DELETE | READ_CONTROL).follow(FollowSymlinks::No).maybe_dir(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    let source = source_parent.open_with(source_name, &options).map_err(CoreError::io)?;
    let metadata = source.metadata().map_err(CoreError::io)?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(CoreError::new(ErrorCode::Unsupported, "only files and directories can be moved"));
    }
    paranoic_windows::rename_handle_no_replace(source.as_handle(), target_parent.as_handle(), target_name.as_ref())
        .map_err(CoreError::io)
}

pub(super) fn stage(storage: &LocalStorage, target: &ResourcePath, job: JobId) -> Result<Box<dyn StagedWrite>> {
    let (parent, name) = storage.parent(target)?;
    let temporary = format!(".paranoic-{}", job.0);
    parent.create_dir(&temporary).map_err(CoreError::io)?;
    // Pin the exact new directory before changing its ACL. No reconstructed path
    // can redirect the ACL update to a replacement object outside this grant.
    let mut options = OpenOptions::new();
    options.access_mode(FILE_GENERIC_READ | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
        .follow(FollowSymlinks::No).maybe_dir(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    let opened = parent.open_with(&temporary, &options).map_err(CoreError::io)?;
    if !opened.metadata().map_err(CoreError::io)?.is_dir() {
        return Err(CoreError::new(ErrorCode::PermissionDenied, "staging is not a directory"));
    }
    let directory = Dir::from_std_file(opened.into_std());
    let private = paranoic_windows::ensure_owner_only_handle(directory.as_handle())
        .and_then(|_| paranoic_windows::is_owner_only_handle(directory.as_handle()));
    if !matches!(private, Ok(true)) {
        drop(directory);
        let _ = parent.remove_dir(&temporary);
        return Err(CoreError::new(ErrorCode::PermissionDenied, "staging folder could not be made private"));
    }
    let mut stage = WindowsStage { parent, directory: Some(directory), temporary, target: name.to_owned(), file: None, finished: false };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    stage.file = Some(stage.directory.as_ref().ok_or_else(internal)?.open_with("payload", &options).map_err(CoreError::io)?);
    Ok(Box::new(stage))
}
fn internal() -> CoreError { CoreError::new(ErrorCode::Internal, "closed Windows staging") }
struct WindowsStage { parent: Dir, directory: Option<Dir>, temporary: String, target: String, file: Option<File>, finished: bool }
impl WindowsStage {
    fn cleanup(&mut self) -> Result<()> {
        self.file.take();
        if let Some(directory) = self.directory.take() {
            let result = directory.remove_file("payload").map_err(CoreError::io);
            // Close directory handle before removing its name (Windows sharing rules).
            drop(directory);
            result?;
        }
        self.parent.remove_dir(&self.temporary).map_err(CoreError::io)?;
        self.finished = true;
        Ok(())
    }
}
impl Write for WindowsStage {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> { self.file.as_mut().ok_or_else(closed_writer)?.write(data) }
    fn flush(&mut self) -> std::io::Result<()> { self.file.as_mut().ok_or_else(closed_writer)?.flush() }
}
impl StagedWrite for WindowsStage {
    fn publish(mut self: Box<Self>) -> Result<()> {
        let mut file = self.file.take().ok_or_else(internal)?;
        file.flush().map_err(CoreError::io)?;
        file.sync_all().map_err(CoreError::io)?;
        drop(file);
        if let Err(error) = self.directory.as_ref().ok_or_else(internal)?.hard_link("payload", &self.parent, &self.target) {
            if error.kind() == std::io::ErrorKind::AlreadyExists { self.cleanup()?; }
            return Err(CoreError::io(error));
        }
        // Once linked, cleanup failure is reported; the published target is never removed.
        self.cleanup()
    }
    fn abort(mut self: Box<Self>) -> Result<()> { self.cleanup() }
}
impl Drop for WindowsStage {
    fn drop(&mut self) {
        if self.finished { return; }
        self.file.take();
        if let Some(directory) = self.directory.take() { let _ = directory.remove_file("payload"); drop(directory); }
        let _ = self.parent.remove_dir(&self.temporary);
    }
}

