//! Plan 1.4, part 3: Cryptomator vaults (formats 7/8) on any storage — local
//! or cloud (owner decision). Every change is prepared by the vault reader and
//! executed as a journaled Core job without replacing existing objects;
//! decrypted data goes to a private spool and is copied out as a new file.
//! Quick Lock and shutdown forget every open vault.
use crate::*;
use crypto_files::crypto_error;
use features::{finish, run_wait, StageSink};
use paranoic_crypto::CryptoError;
use paranoic_formats::{CryptomatorLengthPolicy, CryptomatorListedKind, CryptomatorPlan, CryptomatorPlanStatus, CryptomatorReader, CryptomatorVault};
use std::{sync::atomic::Ordering, time::Duration};
use zeroize::Zeroizing;

const LIFETIME: Duration = Duration::from_secs(4 * 3600);
const LIST_LIMIT: usize = 4096;
/// Unfinished Cryptomator password changes, in the app's private state (B11).
const PENDING_DIR: &str = "cryptomator-pending";
const PENDING_LIMIT: usize = 64;

/// An entry of a Cryptomator listing.
#[derive(Clone, uniffi::Record)]
pub struct NativeContainerEntry { pub name: String, pub folder: bool, pub size: u64 }

#[derive(Clone, uniffi::Record)]
pub struct NativeCryptomatorListing {
    /// `size` of files is the encrypted size (Cryptomator stores no plaintext
    /// length); folders 0.
    pub entries: Vec<NativeContainerEntry>,
    /// Objects that could not be shown (links, foreign files).
    pub ignored: u64,
}

fn wait_job(core: &paranoic_core::Core, job: paranoic_core::JobRecord) -> Result<()> { finish(run_wait(core, job.id)?, 1) }

/// Run `plan` step by step until it completes; a stopped step is Attention.
fn drive(core: &paranoic_core::Core, mut advance: impl FnMut() -> paranoic_core::Result<CryptomatorPlanStatus>) -> Result<()> {
    loop {
        match advance()? {
            CryptomatorPlanStatus::Completed => return Ok(()),
            CryptomatorPlanStatus::Submitted { job, .. } | CryptomatorPlanStatus::Running { job, .. } => { run_wait(core, job)?; }
            _ => return Err(BridgeError::Attention),
        }
    }
}

impl NativeRuntime {
    fn pending_dir(&self) -> std::path::PathBuf { self.state_root.join(PENDING_DIR) }
    /// Keep a password-change plan durably before its first step: between its
    /// steps 2 and 3 the vault has no masterkey file, and after a crash only
    /// this record lets `cryptomator_open` finish the change.
    pub(crate) fn save_pending(&self, plan: &CryptomatorPlan) -> Result<std::path::PathBuf> {
        use std::io::Write;
        let dir = self.pending_dir();
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)] { use std::os::unix::fs::DirBuilderExt; builder.mode(0o700); }
        builder.create(&dir).map_err(|e| BridgeError::from(paranoic_core::CoreError::io(e)))?;
        let path = dir.join(format!("{}.json", plan.plan_id().simple()));
        let temporary = path.with_extension("tmp");
        let bytes = plan.to_json().map_err(crypto_error)?;
        let write = || -> std::io::Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            #[cfg(unix)] std::fs::File::open(&dir)?.sync_all()?;
            Ok(())
        };
        write().map_err(|e| BridgeError::from(paranoic_core::CoreError::io(e)))?;
        Ok(path)
    }
    /// Recorded plans for the vault at `root` (unreadable records are kept for
    /// the owner, never deleted).
    fn pending_for(&self, root: &Location) -> Vec<(std::path::PathBuf, CryptomatorPlan)> {
        let Ok(entries) = std::fs::read_dir(self.pending_dir()) else { return Vec::new() };
        entries.flatten().take(PENDING_LIMIT).filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { return None; }
            let plan = CryptomatorPlan::from_json(&std::fs::read(&path).ok()?).ok()?;
            let on_root = plan.root() == &root.path && plan.steps().iter().all(|s| s.target().storage == root.storage);
            on_root.then_some((path, plan))
        }).collect()
    }
    /// Before opening: finish interrupted password changes of this vault.
    fn finish_pending(&self, root: &Location) -> Result<()> {
        for (_, plan) in self.pending_for(root) {
            drive(&self.core, || plan.advance_detached(&self.core, root.storage))?;
        }
        Ok(())
    }
    /// After a completed change, opened with the NEW password: delete the
    /// masterkey backup that still opens the vault with the old one. The format
    /// first checks that the new file opens this vault with `password`.
    fn remove_old_backups(&self, reader: &CryptomatorReader, root: &Location, password: &Zeroizing<String>) -> Result<()> {
        for (record, plan) in self.pending_for(root) {
            let backup = root.path.join(&format!("masterkey.cryptomator.{}.bkup", plan.plan_id().simple()))?;
            let gone = matches!(self.core.metadata(&Location { storage: root.storage, path: backup }),
                Err(ref e) if e.code == paranoic_core::ErrorCode::NotFound);
            if !gone {
                let removal = reader.prepare_remove_masterkey_backup(&plan, password.clone(), Uuid::new_v4(), || Ok(())).map_err(crypto_error)?;
                drive(&self.core, || reader.advance_plan(&self.core, &removal))?;
            }
            let _ = std::fs::remove_file(record);
        }
        Ok(())
    }
    pub(crate) fn cryptomator(&self, handle: &str) -> Result<Arc<CryptomatorReader>> {
        self.cryptomator_vaults.lock().map_err(lock_error)?.get(handle).cloned().ok_or(BridgeError::Locked)
    }
    fn cryptomator_register(&self, root: Location, password: Zeroizing<String>, epoch: u64) -> Result<String> {
        let storage = self.core.storage(root.storage)?;
        let reader = CryptomatorReader::open(storage, root.path, password, LIFETIME, || Ok(())).map_err(crypto_error)?;
        let handle = Uuid::new_v4().to_string();
        let mut vaults = self.cryptomator_vaults.lock().map_err(lock_error)?;
        if self.secrets_epoch.load(Ordering::SeqCst) != epoch { reader.lock(); return Err(BridgeError::Locked); }
        vaults.insert(handle.clone(), Arc::new(reader));
        Ok(handle)
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Create a new Cryptomator vault (format 8) `name` in `parent` and open it.
    /// Expensive key derivation: call off the UI thread.
    pub fn cryptomator_create(&self, parent: NativeLocation, name: String, password: String) -> Result<String> {
        let password = Zeroizing::new(password);
        let epoch = self.secrets_epoch.load(Ordering::SeqCst);
        let parent = parent.core()?;
        let root = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let bootstrap = CryptomatorVault::prepare_create(Zeroizing::new(password.to_string()), || Ok(())).map_err(crypto_error)?;
        wait_job(&self.core, bootstrap.submit(&self.core, root.clone(), JobId(Uuid::new_v4()))?)?;
        self.cryptomator_register(root, password, epoch)
    }
    /// Open an existing vault folder (the one containing `vault.cryptomator` or,
    /// for format 7, `masterkey.cryptomator`).
    /// An interrupted password change of this vault is finished first; the
    /// new password is then needed, and the old-password backup is removed.
    pub fn cryptomator_open(&self, root: NativeLocation, password: String) -> Result<String> {
        let password = Zeroizing::new(password);
        let epoch = self.secrets_epoch.load(Ordering::SeqCst);
        let root = root.core()?;
        self.finish_pending(&root)?;
        let handle = self.cryptomator_register(root.clone(), password.clone(), epoch)?;
        // Retried on every open until it succeeds; the vault itself is usable.
        if let Ok(reader) = self.cryptomator(&handle) { let _ = self.remove_old_backups(&reader, &root, &password); }
        Ok(handle)
    }
    /// New password for an open vault (B11). `old_password` is checked first;
    /// the new masterkey file is published, the old one kept as a backup
    /// until the new file is verified, then the backup is deleted. Expensive
    /// key derivation several times: call off the UI thread. Copies of the
    /// old masterkey file outside this storage (cloud versions, backups)
    /// still open the vault with the old password.
    pub fn cryptomator_change_password(&self, handle: String, old_password: String, new_password: String) -> Result<()> {
        let (old, new) = (Zeroizing::new(old_password), Zeroizing::new(new_password));
        let reader = self.cryptomator(&handle)?;
        reader.verify_password(old, || Ok(())).map_err(crypto_error)?;
        let plan = reader.prepare_password_change_plan(new.clone(), Uuid::new_v4(), || Ok(())).map_err(crypto_error)?;
        let root = Location { storage: plan.steps()[0].target().storage, path: plan.root().clone() };
        self.save_pending(&plan)?;
        drive(&self.core, || reader.advance_plan(&self.core, &plan))?;
        self.remove_old_backups(&reader, &root, &new)
    }
    pub fn cryptomator_list(&self, handle: String, path: Vec<String>) -> Result<NativeCryptomatorListing> {
        let reader = self.cryptomator(&handle)?;
        let listing = reader.list_directory(&path, LIST_LIMIT, || Ok(())).map_err(crypto_error)?;
        let mut ignored = listing.ignored_objects() as u64;
        let mut entries = Vec::new();
        for e in listing.entries() {
            match e.kind() {
                CryptomatorListedKind::File => entries.push(NativeContainerEntry { name: e.name().to_owned(), folder: false, size: e.ciphertext_bytes().unwrap_or(0) }),
                CryptomatorListedKind::Directory => entries.push(NativeContainerEntry { name: e.name().to_owned(), folder: true, size: 0 }),
                CryptomatorListedKind::UnsupportedLink => ignored += 1,
            }
        }
        Ok(NativeCryptomatorListing { entries, ignored })
    }
    /// Encrypt `source` into folder `path` of the vault under its own name
    /// (never replaces). The source is not changed. Blocking.
    pub fn cryptomator_add(&self, handle: String, path: Vec<String>, source: NativeLocation) -> Result<()> {
        let reader = self.cryptomator(&handle)?;
        let source = source.core()?;
        let name = source.path.parent_and_name()?.1.to_owned();
        let mut input = self.core.open_read(&source)?;
        let before = input.metadata()?;
        if before.kind != paranoic_core::EntryKind::File { return Err(BridgeError::Unsupported); }
        let spool = Self::spool_path()?;
        let staged = self.app.stage(&spool, JobId(Uuid::new_v4()))?;
        let prepared = reader.prepare_file_import(&path, &name, &mut input, Box::new(StageSink(Some(staged))), Default::default(), |_| Ok(()))
            .map_err(crypto_error);
        let result = prepared.and_then(|prepared| {
            if input.metadata()? != before { return Err(BridgeError::Changed); }
            wait_job(&self.core, reader.submit_file_import(&self.core, &prepared,
                Location { storage: self.app.id(), path: spool.clone() }, JobId(Uuid::new_v4()))?)
        });
        self.drop_spool(&spool);
        result
    }
    /// Decrypt `name` in folder `path` into `target_parent` as a new file.
    /// Cryptomator cannot detect removal of whole trailing blocks of a file
    /// (format limitation); everything else is authenticated. Blocking.
    pub fn cryptomator_export(&self, handle: String, path: Vec<String>, name: String, target_parent: NativeLocation) -> Result<()> {
        let reader = self.cryptomator(&handle)?;
        let parent = target_parent.core()?;
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let mut file = path; file.push(name);
        let spool = Self::spool_path()?;
        let staged = self.app.stage(&spool, JobId(Uuid::new_v4()))?;
        let result = reader.decrypt_file(&file, Box::new(StageSink(Some(staged))), Default::default(),
            CryptomatorLengthPolicy::AllowUnauthenticated, |_| Ok(())).map_err(crypto_error).and_then(|_| {
            let job = self.core.submit(Operation::CopyFile { source: Location { storage: self.app.id(), path: spool.clone() },
                target, conflict: paranoic_core::ConflictPolicy::Fail })?;
            wait_job(&self.core, job)
        });
        self.drop_spool(&spool);
        result
    }
    pub fn cryptomator_mkdir(&self, handle: String, path: Vec<String>, name: String) -> Result<()> {
        let reader = self.cryptomator(&handle)?;
        let prepared = reader.prepare_directory(&path, &name, || Ok(())).map_err(crypto_error)?;
        wait_job(&self.core, reader.submit_directory(&self.core, &prepared, JobId(Uuid::new_v4()))?)
    }
    /// Rename within the same folder (never replaces).
    pub fn cryptomator_rename(&self, handle: String, path: Vec<String>, name: String, new_name: String) -> Result<()> {
        let reader = self.cryptomator(&handle)?;
        match reader.prepare_rename(&path, &name, &new_name, || Ok(())) {
            Ok(prepared) => wait_job(&self.core, reader.submit_rename(&self.core, &prepared, JobId(Uuid::new_v4()))?),
            // A file whose name changes shortening class (.c9r <-> .c9s) is a
            // staged multi-step plan; content ciphertext is reused.
            Err(CryptoError::Unsupported) => {
                let plan = reader.prepare_file_reshape(&path, &name, &path, &new_name, Uuid::new_v4(), || Ok(())).map_err(crypto_error)?;
                finish_plan_steps(&self.core, &reader, &plan)
            }
            Err(error) => Err(crypto_error(error)),
        }
    }
    /// Delete a file or a folder with its content (multi-step plan; on a cloud
    /// storage the provider's trash keeps the objects). A stopped step leaves
    /// the job in "needs attention" (plan 1.3).
    pub fn cryptomator_delete(&self, handle: String, path: Vec<String>, name: String) -> Result<()> {
        let reader = self.cryptomator(&handle)?;
        let plan = reader.prepare_delete(&path, &name, Default::default(), Uuid::new_v4(), || Ok(())).map_err(crypto_error)?;
        finish_plan_steps(&self.core, &reader, &plan)
    }
    pub fn cryptomator_lock(&self, handle: String) -> Result<()> {
        crate::vault_io::forget_streams_of(&handle);
        self.forget_vault_pdf(Some(&handle));
        if let Some(reader) = self.cryptomator_vaults.lock().map_err(lock_error)?.remove(&handle) { reader.lock(); }
        Ok(())
    }
}

/// Advance a multi-step plan to the end; a stopped step leaves the job in
/// "needs attention" (plan 1.3), nothing is retried blindly.
fn finish_plan_steps(core: &Core, reader: &CryptomatorReader, plan: &CryptomatorPlan) -> Result<()> {
    loop {
        match reader.advance_plan(core, plan)? {
            CryptomatorPlanStatus::Completed => return Ok(()),
            CryptomatorPlanStatus::Submitted { job, .. } | CryptomatorPlanStatus::Running { job, .. } => { run_wait(core, job)?; }
            _ => return Err(BridgeError::Attention),
        }
    }
}
