//! Bounded create-new trees. Only exact owned entries are removed on failure.
use super::*;
use paranoic_core::{validate_prepared_tree, PreparedTreeFile};

struct Directory {
    handle: Dir,
    parent: Option<(usize, String)>,
}

struct PendingTree {
    parent: Dir,
    temporary: String,
    directories: Vec<Directory>,
    files: Vec<(usize, String)>,
    finished: bool,
}

fn open_directory(parent: &Dir, name: &str) -> Result<Dir> {
    let fd = rustix::fs::openat(parent, name,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY |
        rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty()).map_err(|e| CoreError::io(std::io::Error::from(e)))?;
    Ok(Dir::from_std_file(std::fs::File::from(fd)))
}

impl PendingTree {
    fn populate(&mut self, files: &[PreparedTreeFile], checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        let mut directories = std::collections::BTreeMap::new();
        directories.insert(String::new(), 0_usize);
        let mut builder = DirBuilder::new();
        use cap_std::fs::{DirBuilderExt, OpenOptionsExt};
        builder.mode(0o700);
        for file in files {
            checkpoint()?;
            let (relative_parent, name) = file.path.parent_and_name()?;
            let mut current = 0;
            let mut prefix = String::new();
            if !relative_parent.is_root() {
                for component in relative_parent.as_str().split('/') {
                    if !prefix.is_empty() { prefix.push('/'); }
                    prefix.push_str(component);
                    if let Some(index) = directories.get(&prefix) {
                        current = *index;
                        continue;
                    }
                    let parent = &self.directories[current].handle;
                    parent.create_dir_with(component, &builder).map_err(CoreError::io)?;
                    let handle = match open_directory(parent, component) {
                        Ok(handle) => handle,
                        Err(error) => {
                            // It has no owned children yet. Failure to remove it
                            // takes precedence over the original open error.
                            parent.remove_dir(component).map_err(CoreError::io)?;
                            return Err(error);
                        }
                    };
                    let index = self.directories.len();
                    self.directories.push(Directory { handle, parent: Some((current, component.to_owned())) });
                    directories.insert(prefix.clone(), index);
                    current = index;
                }
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            let mut output = self.directories[current].handle.open_with(name, &options).map_err(CoreError::io)?;
            self.files.push((current, name.to_owned()));
            output.write_all(&file.bytes).map_err(CoreError::io)?;
            output.sync_all().map_err(CoreError::io)?;
        }
        // Children are inserted after parents, so reverse order flushes leaves
        // before the namespace which makes them reachable.
        for directory in self.directories.iter().rev() {
            checkpoint()?;
            sync_directory(&directory.handle)?;
        }
        Ok(())
    }

    fn cleanup(&mut self) -> Result<()> {
        use cap_std::fs::MetadataExt;
        let held = self.directories[0].handle.dir_metadata().map_err(CoreError::io)?;
        let visible = self.parent.symlink_metadata(&self.temporary).map_err(CoreError::io)?;
        if !visible.is_dir() || visible.dev() != held.dev() || visible.ino() != held.ino() {
            // Do not remove another entry under a reused staging name, nor
            // traverse handles after somebody moved our directory elsewhere.
            self.finished = true;
            return Err(CoreError::new(ErrorCode::NeedsAttention, "bootstrap cleanup identity changed"));
        }
        for (index, name) in self.files.iter().rev() {
            match self.directories[*index].handle.remove_file(name) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(CoreError::io(error)),
            }
        }
        for directory in self.directories.iter().rev() {
            if let Some((index, name)) = &directory.parent {
                match self.directories[*index].handle.remove_dir(name) {
                    Ok(()) => {},
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(CoreError::io(error)),
                }
            }
        }
        self.parent.remove_dir(&self.temporary).map_err(CoreError::io)?;
        self.finished = true;
        sync_directory(&self.parent)
    }
}

impl Drop for PendingTree {
    fn drop(&mut self) {
        if !self.finished { let _ = self.cleanup(); }
    }
}

pub(super) fn publish(storage: &LocalStorage, target: &ResourcePath, job: JobId,
    files: &[PreparedTreeFile], checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<()> {
    use cap_std::fs::{DirBuilderExt, MetadataExt};
    storage.writable()?;
    validate_prepared_tree(files)?;
    let (parent, name) = storage.parent(target)?;
    let temporary = format!(".paranoic-{}", job.0);
    checkpoint()?;
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    parent.create_dir_with(&temporary, &builder).map_err(CoreError::io)?;
    let directory = match open_directory(&parent, &temporary) {
        Ok(directory) => directory,
        Err(error) => {
            parent.remove_dir(&temporary).map_err(CoreError::io)?;
            sync_directory(&parent)?;
            return Err(error);
        }
    };
    let metadata = directory.dir_metadata().map_err(CoreError::io)?;
    if !crate::shared::staging_private(&metadata, storage.shared_root) {
        // Ownership is uncertain: do not traverse or remove this directory.
        return Err(CoreError::new(ErrorCode::PermissionDenied, "bootstrap staging is not private"));
    }
    let mut pending = PendingTree { parent, temporary,
        directories: vec![Directory { handle: directory, parent: None }], files: Vec::new(), finished: false };
    let preparation = (|| {
        pending.populate(files, checkpoint)?;
        checkpoint()?;
        let visible = pending.parent.symlink_metadata(&pending.temporary).map_err(CoreError::io)?;
        if !visible.is_dir() || visible.dev() != metadata.dev() || visible.ino() != metadata.ino() {
            return Err(CoreError::new(ErrorCode::NeedsAttention, "bootstrap staging identity changed"));
        }
        crate::shared::rename_noreplace(&pending.parent, pending.temporary.as_str(), &pending.parent,
            name, storage.shared_root).map_err(|e| CoreError::io(std::io::Error::from(e)))?;
        // Rename is the publication boundary, even if parent fsync fails.
        pending.finished = true;
        Ok(())
    })();
    if let Err(error) = preparation {
        pending.cleanup()?;
        return Err(error);
    }
    sync_directory(&pending.parent)
}
