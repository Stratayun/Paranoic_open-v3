//! UI bridge for protected notes (1.9), WebDAV connections (3.5/3.7) and the
//! Paranoic encrypted-folder format (1.8). Blocking calls: invoke off the UI
//! thread. Secrets cross the boundary only as call arguments; nothing here
//! logs or persists plaintext.
use std::{path::PathBuf, sync::{Arc, atomic::Ordering}, time::Duration};
use base64::{engine::general_purpose::STANDARD, Engine};
use paranoic_core::{JobState, Location, Operation, ResourcePath, Storage, StorageId};
use paranoic_formats::{paranoic_v1::{EntryKind, KdfParams, NewFile, NewPlace}, paranoic_vault::{run_plan, EntryRef, ParanoicVault, Spool, VaultPlan}};
use paranoic_notes::{Attachment, ErasureGuarantee, Note, Notebook, NotesError, NotesLimits, NotesSession, NotesState, NotesStore, PasswordStrength, Tab};
use uuid::Uuid;
use crate::{lock_error, space, BridgeError, HostBytes, NativeGrant, NativeLocation, NativeRuntime, NativeSpace, Result};

// ---------------------------------------------------------------- notes

/// Host key service for notes (Android Keystore; none on desktop yet).
/// `guarantee`: 0 password only, 1 device key, 2 hardware key (StrongBox /
/// rollback resistant). `erase_key` irrevocably deletes the key (0 = ok).
#[uniffi::export(with_foreign)]
pub trait NotesKeyHost: Send + Sync {
    fn seal(&self, data: Vec<u8>) -> HostBytes;
    fn open(&self, data: Vec<u8>) -> HostBytes;
    fn erase_key(&self) -> u32;
    fn guarantee(&self) -> u8;
}
struct HostKey(Arc<dyn NotesKeyHost>);
impl paranoic_notes::DeviceKey for HostKey {
    fn seal(&self, data: &[u8]) -> paranoic_notes::Result<Vec<u8>> {
        let r = self.0.seal(data.to_vec()); if r.error == 0 { Ok(r.data) } else { Err(NotesError::Device) }
    }
    fn open(&self, sealed: &[u8]) -> paranoic_notes::Result<Vec<u8>> {
        let r = self.0.open(sealed.to_vec()); if r.error == 0 { Ok(r.data) } else { Err(NotesError::Device) }
    }
    fn destroy(&self) -> paranoic_notes::Result<()> { if self.0.erase_key() == 0 { Ok(()) } else { Err(NotesError::Device) } }
    fn guarantee(&self) -> ErasureGuarantee {
        match self.0.guarantee() { 2 => ErasureGuarantee::HardwareKey, 1 => ErasureGuarantee::DeviceKey, _ => ErasureGuarantee::PasswordOnly }
    }
}

#[derive(Default)]
pub(crate) struct NotesSlot { key: Option<Arc<dyn NotesKeyHost>>, quick: Option<Arc<dyn NotesKeyHost>>, store: Option<NotesStore>, session: Option<NotesSession> }

/// A file kept inside the notes (Р-3): viewable in the notes, never exported.
#[derive(Clone, uniffi::Record)]
pub struct NativeAttachment { pub id: String, pub name: String, pub size: u64 }
/// `attachments` must be passed back unchanged when saving: a note saved
/// without them loses them (the objects are then removed).
#[derive(Clone, uniffi::Record)]
pub struct NativeNote { pub id: String, pub title: String, pub body: String, pub modified_ms: u64, pub attachments: Vec<NativeAttachment> }
#[derive(Clone, uniffi::Record)]
pub struct NativeTab { pub id: String, pub title: String, pub notes: Vec<NativeNote> }
#[derive(Clone, uniffi::Record)]
pub struct NativeNotebook { pub tabs: Vec<NativeTab> }
/// `guarantee`: what Clear can promise here (0 password only, 1 device key, 2 hardware key).
#[derive(Clone, uniffi::Record)]
pub struct NativeNotesStatus { pub set_up: bool, pub unlocked: bool, pub auto_wipe: bool, pub failed_attempts: u32,
    pub wipe_after: u32, pub retry_after_ms: u64, pub guarantee: u8,
    /// A quick-unlock key is configured on this platform / quick unlock is on.
    pub quick_available: bool, pub quick_enabled: bool }

pub(crate) fn notes_error(error: NotesError) -> BridgeError {
    match error {
        NotesError::RateLimited { retry_after_ms } => BridgeError::RateLimited { retry_after_ms },
        NotesError::WrongPassword => BridgeError::WrongPassword,
        NotesError::WeakPassword => BridgeError::WeakPassword,
        NotesError::NotInitialized => BridgeError::NotSetUp,
        NotesError::AlreadyInitialized => BridgeError::Conflict,
        NotesError::Locked | NotesError::QuickUnlockDisabled => BridgeError::Locked,
        NotesError::Limit => BridgeError::Limit,
        NotesError::Corrupt => BridgeError::Changed,
        NotesError::Device => BridgeError::DeviceKey,
        NotesError::Wiped => BridgeError::Wiped,
        NotesError::SamePassword => BridgeError::Invalid,
        NotesError::Io | NotesError::ClearIncomplete => BridgeError::Unavailable,
    }
}
fn guarantee_code(g: ErasureGuarantee) -> u8 { match g { ErasureGuarantee::PasswordOnly => 0, ErasureGuarantee::DeviceKey => 1, ErasureGuarantee::HardwareKey => 2 } }
fn to_native(book: &Notebook) -> NativeNotebook {
    NativeNotebook { tabs: book.tabs.iter().map(|t| NativeTab { id: t.id.clone(), title: t.title.clone(),
        notes: t.notes.iter().map(|n| NativeNote { id: n.id.clone(), title: n.title.clone(), body: n.body.clone(), modified_ms: n.modified_ms,
            attachments: n.attachments.iter().map(|a| NativeAttachment { id: a.id.clone(), name: a.name.clone(), size: a.size }).collect() }).collect() }).collect() }
}
fn from_native(book: NativeNotebook) -> Notebook {
    Notebook { tabs: book.tabs.into_iter().map(|t| Tab { id: t.id, title: t.title,
        notes: t.notes.into_iter().map(|n| Note { id: n.id, title: n.title, body: n.body, modified_ms: n.modified_ms,
            attachments: n.attachments.into_iter().map(|a| Attachment { id: a.id, name: a.name, size: a.size }).collect() }).collect() }).collect() }
}
/// Private directory (0700 on Unix; Windows inherits the owner-only state root).
fn private_dir(path: &PathBuf) -> Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    { use std::os::unix::fs::DirBuilderExt; builder.mode(0o700); }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => Err(BridgeError::Unavailable),
    }
}

impl NativeRuntime {
    /// Run `f` with the open notes session (Locked if none).
    pub(crate) fn with_notes<T>(&self, f: impl FnOnce(&NotesStore, &NotesSession) -> Result<T>) -> Result<T> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = slot.session.take().ok_or(BridgeError::Locked)?;
        let result = self.notes_store(&mut slot).and_then(|store| f(store, &session));
        slot.session = Some(session);
        result
    }
    pub(crate) fn notes_store<'a>(&self, slot: &'a mut NotesSlot) -> Result<&'a NotesStore> {
        if slot.store.is_none() {
            let dir = self.state_root.join("notes");
            private_dir(&dir)?;
            let device = slot.key.clone().map(|k| Arc::new(HostKey(k)) as Arc<dyn paranoic_notes::DeviceKey>);
            let quick = slot.quick.clone().map(|k| Arc::new(HostKey(k)) as Arc<dyn paranoic_notes::DeviceKey>);
            slot.store = Some(NotesStore::open(&dir, device, NotesLimits::default()).map_err(notes_error)?.with_quick_key(quick));
        }
        slot.store.as_ref().ok_or(BridgeError::Unavailable)
    }
    /// Drop decrypted state: notes session and open encrypted folders (Quick Lock, shutdown).
    pub(crate) fn lock_secrets(&self) {
        self.secrets_epoch.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut slot) = self.notes.lock() {
            slot.session = None;
            if let Some(store) = &slot.store { let _ = store.lock(); }
        }
        if let Ok(mut vaults) = self.vaults.lock() { vaults.clear(); }
        crate::vault_io::forget_streams();
        self.forget_vault_pdf(None);
        if let Ok(mut readers) = self.cryptomator_vaults.lock() {
            for (_, reader) in readers.drain() { reader.lock(); }
        }
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Configure the device key before the first notes call (Android Keystore).
    /// The same key must be supplied on every start: a sealed key record is
    /// never opened without it.
    pub fn notes_configure(&self, key: Option<Arc<dyn NotesKeyHost>>) -> Result<()> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        if slot.store.is_some() { return Err(BridgeError::Busy); }
        slot.key = key;
        Ok(())
    }
    /// Quick Lock: forget the notes session and every open encrypted folder.
    pub fn lock_all_secrets(&self) { self.lock_secrets(); }
    pub fn notes_status(&self) -> Result<NativeNotesStatus> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let unlocked = slot.session.is_some();
        let quick_available = slot.quick.is_some();
        let store = self.notes_store(&mut slot)?;
        let set_up = store.state().map_err(notes_error)? == NotesState::Locked;
        Ok(NativeNotesStatus { set_up, unlocked: unlocked && set_up, auto_wipe: store.auto_wipe_enabled().map_err(notes_error)?,
            failed_attempts: store.failed_attempts().map_err(notes_error)?, wipe_after: paranoic_notes::AUTO_WIPE_ATTEMPTS,
            retry_after_ms: store.retry_after().map_err(notes_error)?.as_millis() as u64, guarantee: guarantee_code(store.guarantee()),
            quick_available, quick_enabled: store.quick_unlock_enabled().map_err(notes_error)? })
    }
    /// Configure the platform's quick-unlock key (biometrics; Android: a
    /// Keystore key usable only after user authentication) before any notes
    /// call. Clear and auto-wipe destroy it too (У-27).
    pub fn notes_configure_quick(&self, key: Option<Arc<dyn NotesKeyHost>>) -> Result<()> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        if slot.store.is_some() { return Err(BridgeError::Busy); }
        slot.quick = key;
        Ok(())
    }
    /// Turn quick unlock on (notes must be unlocked; the host has just
    /// authenticated the user so its key can seal). Off by default.
    pub fn notes_quick_enable(&self) -> Result<()> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let quick = slot.quick.clone().ok_or(BridgeError::Unsupported)?;
        self.notes_store(&mut slot)?;
        let (Some(store), Some(session)) = (slot.store.as_ref(), slot.session.as_ref()) else { return Err(BridgeError::Locked) };
        store.enable_quick_unlock(session, &HostKey(quick)).map_err(notes_error)
    }
    pub fn notes_quick_disable(&self) -> Result<()> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        self.notes_store(&mut slot)?.disable_quick_unlock().map_err(notes_error)
    }
    /// Unlock with the quick-unlock key (call right after the host's biometric
    /// prompt succeeded). Failed attempts here do not count toward auto-wipe.
    pub fn notes_quick_unlock(&self) -> Result<()> {
        let epoch = self.secrets_epoch.load(Ordering::SeqCst);
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let quick = slot.quick.clone().ok_or(BridgeError::Unsupported)?;
        let session = self.notes_store(&mut slot)?.unlock_quick(&HostKey(quick)).map_err(notes_error)?;
        if self.secrets_epoch.load(Ordering::SeqCst) != epoch { return Err(BridgeError::Locked); }
        slot.session = Some(session);
        Ok(())
    }
    /// 0 weak (refused), 1 fair, 2 strong.
    pub fn notes_password_strength(&self, password: String) -> u8 {
        let password = zeroize::Zeroizing::new(password);
        match paranoic_notes::password_strength(&password) { PasswordStrength::Weak => 0, PasswordStrength::Fair => 1, PasswordStrength::Strong => 2 }
    }
    pub fn notes_create(&self, password: String) -> Result<()> {
        let password = zeroize::Zeroizing::new(password);
        let epoch = self.secrets_epoch.load(Ordering::SeqCst);
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = self.notes_store(&mut slot)?.create(&password).map_err(notes_error)?;
        if self.secrets_epoch.load(Ordering::SeqCst) != epoch { return Err(BridgeError::Locked); }
        slot.session = Some(session);
        Ok(())
    }
    pub fn notes_unlock(&self, password: String) -> Result<()> {
        let password = zeroize::Zeroizing::new(password);
        let epoch = self.secrets_epoch.load(Ordering::SeqCst);
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let (session, duress) = match self.notes_store(&mut slot)?.unlock_detailed(&password) {
            Ok(outcome) => outcome,
            Err(paranoic_notes::NotesError::Wiped) => {
                // У-68: auto-wipe destroyed the personal folder's secret with the
                // notes; its files can no longer be read — remove them now.
                drop(slot);
                self.vaults.lock().map_err(lock_error)?.clear();
                let _ = self.remove_safe_folder();
                return Err(BridgeError::Wiped);
            }
            Err(error) => return Err(notes_error(error)),
        };
        if duress {
            // Plan 5.2: notes were erased and set up empty; the personal
            // folder (its secret is gone with them) is removed the same way,
            // without telling anyone. Open vaults are dropped as on Quick Lock.
            drop(slot);
            self.vaults.lock().map_err(lock_error)?.clear();
            let _ = self.remove_safe_folder();
            slot = self.notes.lock().map_err(lock_error)?;
        }
        if self.secrets_epoch.load(Ordering::SeqCst) != epoch { return Err(BridgeError::Locked); }
        slot.session = Some(session);
        Ok(())
    }
    /// Plan 5.2: set (`Some`) or remove (`None`) the duress password; open notes required.
    pub fn notes_set_duress(&self, password: Option<String>) -> Result<()> {
        let password = password.map(zeroize::Zeroizing::new);
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = slot.session.take().ok_or(BridgeError::Locked)?;
        let result = self.notes_store(&mut slot).and_then(|s| s.set_duress(&session, password.as_deref().map(String::as_str)).map_err(notes_error));
        slot.session = Some(session);
        result
    }
    pub fn notes_duress_enabled(&self) -> Result<bool> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = slot.session.take().ok_or(BridgeError::Locked)?;
        let result = self.notes_store(&mut slot).and_then(|s| s.duress_enabled(&session).map_err(notes_error));
        slot.session = Some(session);
        result
    }
    pub fn notes_lock(&self) -> Result<()> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        slot.session = None;
        if let Some(store) = &slot.store { store.lock().map_err(notes_error)?; }
        Ok(())
    }
    pub fn notes_read(&self) -> Result<NativeNotebook> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = slot.session.take().ok_or(BridgeError::Locked)?;
        let result = self.notes_store(&mut slot)?.read_notebook(&session).map_err(notes_error);
        slot.session = Some(session);
        Ok(to_native(&result?))
    }
    pub fn notes_save(&self, notebook: NativeNotebook) -> Result<()> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = slot.session.take().ok_or(BridgeError::Locked)?;
        let result = self.notes_store(&mut slot)?.save_notebook(&session, &from_native(notebook)).map_err(notes_error);
        slot.session = Some(session);
        result
    }
    /// New password and a new notes key (Р-1). Quick unlock is turned off and
    /// has to be enabled again. The session continues with the new key; after a
    /// failure past the key switch the notes are locked (unlock with the new password).
    /// Store a file inside the notes (encrypted with the notes key). Put the
    /// returned reference into a note and save; unsaved ones are removed after lock.
    pub fn notes_attachment_add(&self, name: String, data: Vec<u8>) -> Result<NativeAttachment> {
        let data = zeroize::Zeroizing::new(data);
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = slot.session.take().ok_or(BridgeError::Locked)?;
        let result = self.notes_store(&mut slot).and_then(|store| store.add_attachment(&session, &name, &data).map_err(notes_error));
        slot.session = Some(session);
        let a = result?;
        Ok(NativeAttachment { id: a.id, name: a.name, size: a.size })
    }
    /// Plaintext of an attachment, for viewing inside the notes only (never export it).
    pub fn notes_attachment_read(&self, id: String) -> Result<Vec<u8>> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = slot.session.take().ok_or(BridgeError::Locked)?;
        let result = self.notes_store(&mut slot).and_then(|store| store.read_attachment(&session, &id).map_err(notes_error));
        slot.session = Some(session);
        Ok(result?.to_vec())
    }
    pub fn notes_change_password(&self, old: String, new: String) -> Result<()> {
        let (old, new) = (zeroize::Zeroizing::new(old), zeroize::Zeroizing::new(new));
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = slot.session.take().ok_or(BridgeError::Locked)?;
        let result = self.notes_store(&mut slot).and_then(|store| store.change_password(&session, &old, &new).map_err(notes_error));
        match result {
            Ok(fresh) => { slot.session = Some(fresh); Ok(()) }
            Err(error) => { slot.session = Some(session); Err(error) }
        }
    }
    pub fn notes_set_auto_wipe(&self, enabled: bool) -> Result<()> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        let session = slot.session.take().ok_or(BridgeError::Locked)?;
        let result = self.notes_store(&mut slot)?.set_auto_wipe(&session, enabled).map_err(notes_error);
        slot.session = Some(session);
        result
    }
    /// Clear: everything in the notes is destroyed; returns the guarantee code.
    pub fn notes_clear(&self) -> Result<u8> {
        let mut slot = self.notes.lock().map_err(lock_error)?;
        slot.session = None;
        let guarantee = self.notes_store(&mut slot)?.clear(None).map_err(notes_error)?;
        crate::diagnostics::clear();
        Ok(guarantee_code(guarantee))
    }
}

// ---------------------------------------------------------------- «Сейф» (plan 5.1)

impl NativeRuntime {
    /// Encrypted files of the personal folder (already unreadable once the
    /// notes secret is gone).
    fn remove_safe_folder(&self) -> Result<()> {
        let location = Location { storage: self.app.id(), path: ResourcePath::parse(SAFE_DIR)? };
        match self.core.metadata(&location) {
            Ok(_) => {
                let job = self.core.submit(Operation::DeleteDirectory { target: location })?;
                finish(run_wait(&self.core, job.id)?, 1)
            }
            Err(error) if error.code == paranoic_core::ErrorCode::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// The personal folder: a 1.8 vault in the app's own folder, hidden from the
/// file list and never synced (".pnc-", a service name; ".paranoic-" is
/// reserved for adapter staging). Its password is a random secret kept
/// sealed beside the notes, so the notes password opens both.
pub(crate) const SAFE_DIR: &str = ".pnc-safe";
/// The secret is 256 random bits: the minimum Argon2 cost is enough.
const SAFE_KDF: KdfParams = KdfParams { memory_kib: 65_536, iterations: 1, lanes: 1 };

#[uniffi::export]
impl NativeRuntime {
    /// Open (on first use: create) the personal folder with the open notes
    /// session. Returns a vault handle for the `vault_*` calls; Quick Lock and
    /// locking the notes end it like any other vault.
    pub fn safe_open(&self) -> Result<String> {
        let epoch = self.secrets_epoch.load(Ordering::SeqCst);
        let existing = self.with_notes(|store, session| store.safe_secret(session, false).map_err(notes_error))?;
        let secret = match existing {
            Some(secret) => secret,
            None => {
                // У-68: no secret means the notes were erased or set up anew; a
                // folder left from before (wipe, interrupted clear) is unreadable
                // for good. Remove it before making a new one. Never on a
                // wrong password: only when the secret does not exist at all.
                self.remove_safe_folder()?;
                self.with_notes(|store, session| store.safe_secret(session, true).map_err(notes_error))?.ok_or(BridgeError::Unavailable)?
            }
        };
        let password = zeroize::Zeroizing::new(secret.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let location = Location { storage: self.app.id(), path: ResourcePath::parse(SAFE_DIR)? };
        let vault = match self.core.metadata(&location) {
            Ok(_) => ParanoicVault::open(&self.core, location, password.as_bytes(), &mut ok).map_err(vault_error)?,
            Err(error) if error.code == paranoic_core::ErrorCode::NotFound => {
                let (plan, vault) = ParanoicVault::plan_create(location, password.as_bytes(), SAFE_KDF, true, &mut ok)?;
                self.run(&plan)?;
                vault
            }
            Err(error) => return Err(error.into()),
        };
        self.register(vault, epoch)
    }
    /// «Очистить всё»: notes Clear destroys the folder's secret (the folder can
    /// no longer be decrypted), then its encrypted files are removed. Returns
    /// the notes guarantee code. A failed removal leaves only unreadable data.
    pub fn safe_clear(&self) -> Result<u8> {
        self.lock_secrets();
        let guarantee = self.notes_clear()?;
        self.remove_safe_folder()?;
        Ok(guarantee)
    }
}

// ---------------------------------------------------------------- WebDAV

const CLOUD_PREFIX: &str = "webdav:";
/// Private temporary files in the app workspace: ".pnc-" because ".paranoic-"
/// is reserved for adapter staging; hidden from listings, swept at start.
pub(crate) const SPOOL_PREFIX: &str = ".pnc-spool-";
/// Exactly the names `spool_path` creates (prefix + 32 hex digits, optional
/// `-m` for a manifest spool): the start sweep must never remove a user
/// object that only shares the prefix (external audit, 2026-11-14).
pub(crate) fn is_spool(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(SPOOL_PREFIX) else { return false };
    let id = rest.strip_suffix("-m").unwrap_or(rest);
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
/// Remove only orphaned spools after opening/recovering the journal.
pub(crate) fn sweep_spools_at_start(app: &paranoic_local::LocalStorage, core: &paranoic_core::Core) {
    let Ok(keep) = crate::spool::referenced(core, app.id()) else { return };
    let Ok(entries) = app.list(&ResourcePath::root(), 100_000) else { return };
    for entry in entries {
        if entry.location.path.parent_and_name().is_ok_and(|(_, n)| is_spool(n) && !keep.contains(n)) {
            remove_tree(app, &entry.location.path, entry.metadata.kind, 0);
        }
    }
}
/// Spool files and folders (archive extraction) left by a crash; bounded depth.
pub(crate) fn remove_tree(app: &paranoic_local::LocalStorage, path: &ResourcePath, kind: paranoic_core::EntryKind, depth: usize) {
    if kind != paranoic_core::EntryKind::Directory { let _ = app.delete_file(path); return; }
    if depth > 64 { return; }
    if let Ok(children) = app.list(path, 100_000) {
        for child in children { remove_tree(app, &child.location.path, child.metadata.kind, depth + 1); }
    }
    let _ = app.delete_empty_directory(path);
}
pub(crate) fn is_cloud(grant: &NativeGrant) -> bool { !grant.android_tree && grant.reference.starts_with(CLOUD_PREFIX) }

/// Result of connecting a server: persist `grant` in the protected grant store
/// (its token is sealed with the device record key), show `space`.
#[derive(Clone, uniffi::Record)]
pub struct NativeCloudAttached { pub grant: NativeGrant, pub space: NativeSpace }

#[derive(serde::Serialize, serde::Deserialize)]
struct CloudReference { url: String, user: String, token: String }

fn webdav_base(url: &str) -> Result<String> {
    let url = url.trim();
    let base = if url.ends_with('/') { url.to_owned() } else { format!("{url}/") };
    if !(base.starts_with("https://") || base.starts_with("http://127.0.0.1") || base.starts_with("http://localhost")) {
        return Err(BridgeError::Denied); // plain http only for a loopback server
    }
    Ok(base)
}

impl NativeRuntime {
    fn cloud_storage(&self, id: StorageId, url: &str, user: &str, token: &str) -> Result<Arc<paranoic_cloud::CloudStorage>> {
        let base = webdav_base(url)?;
        let loopback = base.starts_with("http://");
        let http = if loopback { paranoic_cloud::Http::new_loopback_http()? } else { paranoic_cloud::Http::new()? };
        // A URL alone does not confirm recoverable DELETE. The server is asked on
        // every connect (own server header / Nextcloud trashbin); without a
        // positive answer deletion and moves that delete the source stay off (У-35).
        let dav = paranoic_cloud::WebDav::new(http, &base, paranoic_cloud::WebDavCredentials {
            username: user.to_owned(), password: zeroize::Zeroizing::new(token.to_owned()) }, false)?;
        let trash = dav.probe_trash();
        let dav = dav.with_server_trash(trash);
        let spool = self.state_root.join("cloud-spool");
        private_dir(&spool)?;
        let spool = spool.join(id.0.simple().to_string());
        private_dir(&spool)?;
        let storage = paranoic_cloud::CloudStorage::open(id, Arc::new(dav), paranoic_cloud::CloudId::parse("/")?, &spool)?;
        // Not registered with Core yet, so no upload of this storage is active:
        // crash leftovers can go (У-39). Best effort; failures stay counted.
        let _ = storage.purge_spool();
        Ok(Arc::new(storage))
    }
    /// Spool folders of cloud grants that are no longer saved (detached): nothing
    /// will ever connect them again, so their crash leftovers are purged here.
    fn purge_orphan_cloud_spools(&self, saved: &[NativeGrant]) {
        let root = self.state_root.join("cloud-spool");
        let Ok(folders) = std::fs::read_dir(&root) else { return };
        for folder in folders.flatten() {
            if !folder.file_type().is_ok_and(|kind| kind.is_dir()) { continue; }
            let name = folder.file_name().to_string_lossy().into_owned();
            let in_use = saved.iter().any(|g| crate::grant_id(&g.id).is_ok_and(|id| id.0.simple().to_string() == name));
            if !in_use { let _ = paranoic_cloud::purge_spool_dir(&folder.path()); }
        }
    }
    fn present_cloud(&self, grant: &NativeGrant, storage: Arc<paranoic_cloud::CloudStorage>, label: String) -> Result<NativeSpace> {
        let value = space(storage.as_ref(), label);
        self.core.register_storage(storage.clone())?;
        self.clouds.lock().map_err(lock_error)?.insert(storage.id(), storage);
        let mut spaces = self.spaces.lock().map_err(lock_error)?;
        spaces.retain(|s| s.id != grant.id);
        spaces.push(value.clone());
        Ok(value)
    }
}

fn cloud_label(url: &str) -> String {
    url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("WebDAV").to_owned()
}

#[uniffi::export]
impl NativeRuntime {
    /// Connect an own/WebDAV server (https; plain http only to loopback). The
    /// returned grant must be stored before the next start; the token inside
    /// is sealed with the device record key and never stored in plain text.
    pub fn attach_webdav(&self, url: String, user: String, token: String) -> Result<NativeCloudAttached> {
        let token = zeroize::Zeroizing::new(token);
        if !self.sealer.available() { return Err(BridgeError::DeviceKey); }
        let id = Uuid::new_v4();
        let storage = self.cloud_storage(StorageId(id), &url, &user, &token)?;
        let sealed = self.sealer.seal(token.as_bytes().to_vec());
        if sealed.error != 0 { return Err(BridgeError::DeviceKey); }
        let reference = serde_json::to_string(&CloudReference { url: webdav_base(&url)?, user, token: STANDARD.encode(&sealed.data) })
            .map_err(|_| BridgeError::Invalid)?;
        let grant = NativeGrant { id: id.to_string(), reference: format!("{CLOUD_PREFIX}{reference}"), android_tree: false };
        let mut grants = self.grants.lock().map_err(lock_error)?;
        if grants.len() >= 128 { return Err(BridgeError::Limit); }
        let label = cloud_label(&url);
        let space = self.present_cloud(&grant, storage, label)?;
        grants.push(grant.clone());
        Ok(NativeCloudAttached { grant, space })
    }
    /// Connect saved servers (network; call after start, off the UI thread).
    /// Unreachable servers stay saved and are skipped; returns those connected now.
    pub fn reconnect_cloud(&self) -> Result<Vec<NativeSpace>> {
        let saved: Vec<NativeGrant> = self.grants.lock().map_err(lock_error)?.iter().filter(|g| is_cloud(g)).cloned().collect();
        self.purge_orphan_cloud_spools(&saved);
        let mut connected = Vec::new();
        for grant in saved {
            if self.spaces.lock().map_err(lock_error)?.iter().any(|s| s.id == grant.id) { continue; }
            let Ok(reference) = serde_json::from_str::<CloudReference>(&grant.reference[CLOUD_PREFIX.len()..]) else { continue };
            let Ok(sealed) = STANDARD.decode(reference.token.as_bytes()) else { continue };
            let opened = self.sealer.open(sealed);
            if opened.error != 0 { continue; }
            // A non-UTF-8 token is skipped; its bytes are erased, not kept inside the error.
            let token = match String::from_utf8(opened.data) {
                Ok(token) => zeroize::Zeroizing::new(token),
                Err(error) => { zeroize::Zeroize::zeroize(&mut error.into_bytes()); continue }
            };
            let Ok(id) = crate::grant_id(&grant.id) else { continue };
            let Ok(storage) = self.cloud_storage(id, &reference.url, &reference.user, &token) else { continue };
            if let Ok(space) = self.present_cloud(&grant, storage, cloud_label(&reference.url)) { connected.push(space); }
        }
        Ok(connected)
    }
}

// ---------------------------------------------------------------- 1.8 encrypted folders

#[derive(Clone, uniffi::Record)]
pub struct NativeVaultEntry { pub name: String, pub folder: bool, pub size: u64, pub modified_ms: u64 }
#[derive(Clone, uniffi::Record)]
pub struct NativeVaultListing { pub entries: Vec<NativeVaultEntry>, pub damaged: u64 }

const PLAN_TIMEOUT: Duration = Duration::from_secs(3600);
fn kdf() -> KdfParams { if cfg!(target_os = "android") { KdfParams::PHONE } else { KdfParams::DESKTOP } }
fn ok() -> paranoic_crypto::Result<()> { Ok(()) }

/// Per-device anchors of encrypted folders (format §9): the highest manifest
/// sequence seen per vault, in the app's private state, never synced or
/// exported. One line per vault: hex id, space, sequence. Replaced atomically.
struct FileAnchor { path: PathBuf }
static ANCHORS: std::sync::Mutex<()> = std::sync::Mutex::new(());
const ANCHOR_LIMIT: usize = 100_000;
impl FileAnchor {
    fn read(&self) -> paranoic_core::Result<std::collections::BTreeMap<String, u64>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
            Err(error) => return Err(paranoic_core::CoreError::io(error)),
        };
        let mut out = std::collections::BTreeMap::new();
        for line in text.lines().take(ANCHOR_LIMIT) {
            let Some((id, sequence)) = line.split_once(' ') else { continue };
            if let Ok(sequence) = sequence.parse() { out.insert(id.to_owned(), sequence); }
        }
        Ok(out)
    }
}
fn anchor_id(vault_id: &[u8; 16]) -> String { vault_id.iter().map(|b| format!("{b:02x}")).collect() }
impl paranoic_formats::paranoic_vault::ManifestAnchor for FileAnchor {
    fn get(&self, vault_id: &[u8; 16]) -> paranoic_core::Result<Option<u64>> {
        let _guard = ANCHORS.lock().map_err(|_| paranoic_core::CoreError::new(paranoic_core::ErrorCode::Internal, "anchor lock"))?;
        Ok(self.read()?.get(&anchor_id(vault_id)).copied())
    }
    fn raise(&self, vault_id: &[u8; 16], sequence: u64) -> paranoic_core::Result<()> {
        let _guard = ANCHORS.lock().map_err(|_| paranoic_core::CoreError::new(paranoic_core::ErrorCode::Internal, "anchor lock"))?;
        let mut all = self.read()?;
        let id = anchor_id(vault_id);
        if all.get(&id).is_some_and(|seen| *seen >= sequence) { return Ok(()); }
        all.insert(id, sequence);
        let text: String = all.iter().map(|(id, seq)| format!("{id} {seq}\n")).collect();
        let temporary = self.path.with_extension("tmp");
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            let mut file = options.open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.path)
        };
        write().map_err(paranoic_core::CoreError::io)
    }
}
pub(crate) fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }
pub(crate) fn finish(records: Vec<paranoic_core::JobRecord>, steps: usize) -> Result<()> {
    match records.last() {
        Some(last) if records.len() == steps && last.state == JobState::Succeeded => Ok(()),
        Some(last) => Err(last.error.clone().map(BridgeError::from).unwrap_or(BridgeError::Attention)),
        None => Err(BridgeError::Unavailable),
    }
}

/// 1.8 header unlock: a wrong password or recovery key is reported as such.
pub(crate) fn vault_error(error: paranoic_core::CoreError) -> BridgeError {
    if paranoic_formats::paranoic_vault::is_wrong_key(&error) { crate::diagnostics::record(&error); BridgeError::WrongPassword } else { error.into() }
}

impl NativeRuntime {
    pub(crate) fn vault(&self, handle: &str) -> Result<Arc<ParanoicVault>> {
        self.vaults.lock().map_err(lock_error)?.get(handle).cloned().ok_or(BridgeError::Locked)
    }
    /// Quick Lock (or lock) removes the vault from the open table; an
    /// operation that started before still holds the keys. Checked before it
    /// publishes anything, so nothing is written after the lock (audit, 2026-11-14).
    pub(crate) fn still_open(&self, handle: &str, vault: &ParanoicVault) -> Result<()> {
        match self.vaults.lock().map_err(lock_error)?.get(handle) {
            Some(open) if std::ptr::eq(Arc::as_ptr(open), vault) => Ok(()),
            _ => Err(BridgeError::Locked),
        }
    }
    pub(crate) fn run(&self, plan: &VaultPlan) -> Result<()> { finish(run_plan(&self.core, plan, PLAN_TIMEOUT)?, plan.steps.len()) }
    /// Run a write plan of an encrypted folder through its version manifest
    /// (format §9): the manifest is prepared next to `spool` and removed after.
    pub(crate) fn run_vault(&self, handle: &str, vault: &ParanoicVault, plan: VaultPlan, spool: &ResourcePath) -> Result<()> {
        let manifest_spool = paranoic_formats::paranoic_vault::manifest_spool_path(spool)?;
        let result = vault.with_manifest(&self.core, plan, Spool { storage: self.app.as_ref(), path: spool })
            .map_err(BridgeError::from).and_then(|prepared| {
                self.still_open(handle, vault)?;
                self.run(&prepared)?;
                Ok(vault.confirm(&prepared)?)
            })
            // Settle at once: an older version stops being acceptable (§9);
            // each published manifest is pinned in the anchor before success.
            .and_then(|()| match vault.plan_settle(&self.core, Spool { storage: self.app.as_ref(), path: spool })? {
                Some(settle) => { self.run(&settle)?; Ok(vault.confirm(&settle)?) }
                None => Ok(()),
            });
        self.drop_spool(&manifest_spool);
        result
    }
    pub(crate) fn spool_path() -> Result<ResourcePath> { Ok(ResourcePath::parse(format!("{SPOOL_PREFIX}{}", Uuid::new_v4().simple()))?) }
    /// Remove a block-1 spool file (decrypted export, re-encrypted import, 1.8
    /// object). Its data is always derivable from a source that still exists,
    /// so removing it even after an unknown job outcome loses nothing; unlike
    /// `PublicationSpool` (block 2), where the spool is the only copy.
    /// Decrypted bytes must not linger (У-45): a few retries for transient
    /// locks (Windows: another open handle), then cut the contents so nothing
    /// readable stays; an empty leftover is removed by the start sweep.
    pub(crate) fn drop_spool(&self, path: &ResourcePath) {
        for attempt in 0..3u32 {
            match self.app.delete_file(path) {
                Ok(()) => return,
                Err(error) if error.code == paranoic_core::ErrorCode::NotFound => return,
                Err(_) if attempt < 2 => std::thread::sleep(Duration::from_millis(50 << attempt)),
                Err(_) => {}
            }
        }
        let file = self.workspace_root.join(path.as_str());
        // Private 0700/owner-only folder and a random name; never follow a link.
        if std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_file()) {
            if let Ok(handle) = std::fs::OpenOptions::new().write(true).open(&file) {
                let _ = handle.set_len(0);
                let _ = handle.sync_all();
            }
        }
        let _ = self.app.delete_file(path);
    }
    fn register(&self, vault: ParanoicVault, epoch: u64) -> Result<String> {
        // Whole-vault rollback is detected against what this device has seen (§9).
        let vault = vault.with_anchor(Arc::new(FileAnchor { path: self.state_root.join("vault-anchors") }));
        let handle = Uuid::new_v4().to_string();
        let mut vaults = self.vaults.lock().map_err(lock_error)?;
        if self.secrets_epoch.load(Ordering::SeqCst) != epoch { return Err(BridgeError::Locked); }
        vaults.insert(handle.clone(), Arc::new(vault));
        Ok(handle)
    }
}

#[uniffi::export]
impl NativeRuntime {
    /// Create a new encrypted folder `name` in `parent` and open it; returns a handle.
    pub fn vault_create(&self, parent: NativeLocation, name: String, password: String) -> Result<String> {
        let password = zeroize::Zeroizing::new(password);
        let epoch = self.secrets_epoch.load(Ordering::SeqCst);
        let parent = parent.core()?;
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let (plan, vault) = ParanoicVault::plan_create(target, password.as_bytes(), kdf(), true, &mut ok)?;
        self.run(&plan)?;
        self.register(vault, epoch)
    }
    pub fn vault_open(&self, root: NativeLocation, password: String) -> Result<String> {
        let password = zeroize::Zeroizing::new(password);
        let epoch = self.secrets_epoch.load(Ordering::SeqCst);
        let vault = ParanoicVault::open(&self.core, root.core()?, password.as_bytes(), &mut ok).map_err(vault_error)?;
        self.register(vault, epoch)
    }
    /// Open with the recovery key text shown when it was created (B11).
    pub fn vault_open_recovery(&self, root: NativeLocation, recovery: String) -> Result<String> {
        let recovery = zeroize::Zeroizing::new(recovery);
        let epoch = self.secrets_epoch.load(Ordering::SeqCst);
        let key = paranoic_formats::paranoic_v1::parse_recovery_text(&recovery).map_err(|_| BridgeError::Invalid)?;
        let vault = ParanoicVault::open_recovery(&self.core, root.core()?, &key).map_err(vault_error)?;
        self.register(vault, epoch)
    }
    /// New password for an open encrypted folder (B11). `old_password` is
    /// checked against the stored header first. Two key derivations: call off
    /// the UI thread. Copies of the old header in cloud versions or backups
    /// still open with the old password.
    pub fn vault_change_password(&self, handle: String, old_password: String, new_password: String) -> Result<()> {
        let (old, new) = (zeroize::Zeroizing::new(old_password), zeroize::Zeroizing::new(new_password));
        let vault = self.vault(&handle)?;
        let spool = Self::spool_path()?;
        let result = vault.plan_change_password(&self.core, old.as_bytes(), new.as_bytes(), Spool { storage: self.app.as_ref(), path: &spool }, &mut ok)
            .map_err(vault_error).and_then(|plan| self.run(&plan));
        self.drop_spool(&spool);
        result
    }
    /// Create (or replace) the recovery key after checking `password` (B11).
    /// Returns its text: show it ONCE and do not store it; a replaced key
    /// stops working.
    pub fn vault_set_recovery(&self, handle: String, password: String) -> Result<String> {
        let password = zeroize::Zeroizing::new(password);
        let vault = self.vault(&handle)?;
        let spool = Self::spool_path()?;
        let result = vault.plan_set_recovery(&self.core, password.as_bytes(), Spool { storage: self.app.as_ref(), path: &spool }, &mut ok)
            .map_err(vault_error).and_then(|(plan, text)| { self.run(&plan)?; Ok(text.to_string()) });
        self.drop_spool(&spool);
        result
    }
    pub fn vault_lock(&self, handle: String) -> Result<()> { crate::vault_io::forget_streams_of(&handle); self.forget_vault_pdf(Some(&handle)); self.vaults.lock().map_err(lock_error)?.remove(&handle); Ok(()) }
    pub fn vault_list(&self, handle: String, path: Vec<String>) -> Result<NativeVaultListing> {
        let vault = self.vault(&handle)?;
        let folder = vault.resolve(&self.core, &path)?;
        let listing = vault.list(&self.core, &folder)?;
        Ok(NativeVaultListing { damaged: listing.damaged as u64, entries: listing.entries.into_iter()
            .map(|e| NativeVaultEntry { name: e.name, folder: e.folder, size: e.size, modified_ms: e.modified_ms }).collect() })
    }
    /// Encrypt a file from any connected space into the folder at `path`.
    pub fn vault_add(&self, handle: String, path: Vec<String>, source: NativeLocation) -> Result<()> {
        let vault = self.vault(&handle)?;
        let folder = vault.resolve(&self.core, &path)?;
        let source = source.core()?;
        let mut input = self.core.open_read(&source)?;
        let meta = input.metadata()?;
        if meta.kind != paranoic_core::EntryKind::File { return Err(BridgeError::Unsupported); }
        let name = source.path.parent_and_name()?.1.to_owned();
        let spool = Self::spool_path()?;
        let prepared = vault.prepare_file(&folder, NewFile { name: &name, modified_ms: meta.modified_unix_ms.unwrap_or_else(now_ms), size: meta.bytes },
            &mut input, Spool { storage: self.app.as_ref(), path: &spool }, &mut |_| Ok(()));
        let result = prepared.map_err(BridgeError::from).and_then(|p| {
            if input.metadata()? != meta || self.core.metadata(&source)? != meta { return Err(BridgeError::Changed); }
            self.run_vault(&handle, &vault, vault.plan_add(&p), &spool)
        });
        self.drop_spool(&spool);
        result
    }
    /// Decrypt `name` (after full verification) into `target_parent` as a new file.
    pub fn vault_export(&self, handle: String, path: Vec<String>, name: String, target_parent: NativeLocation) -> Result<()> {
        let vault = self.vault(&handle)?;
        let folder = vault.resolve(&self.core, &path)?;
        let parent = target_parent.core()?;
        let target = Location { storage: parent.storage, path: parent.path.join(&name)? };
        let spool = Self::spool_path()?;
        let staged = self.app.stage(&spool, paranoic_core::JobId(Uuid::new_v4()))?;
        let read = vault.read_file(&self.core, &folder, &name, Box::new(StageSink(Some(staged))), u64::MAX, &mut |_| Ok(()));
        let result = read.map_err(BridgeError::from).and_then(|_| {
            // Locked meanwhile: the decrypted spool is dropped, not published.
            self.still_open(&handle, &vault)?;
            let source = Location { storage: self.app.id(), path: spool.clone() };
            let job = self.core.submit(Operation::CopyFile { source, target, conflict: paranoic_core::ConflictPolicy::Fail })?;
            finish(run_wait(&self.core, job.id)?, 1)
        });
        self.drop_spool(&spool);
        result
    }
    pub fn vault_mkdir(&self, handle: String, path: Vec<String>, name: String) -> Result<()> {
        let vault = self.vault(&handle)?;
        let parent = vault.resolve(&self.core, &path)?;
        let spool = Self::spool_path()?;
        let result = vault.prepare_folder(&self.core, &parent, &name, now_ms(), Spool { storage: self.app.as_ref(), path: &spool })
            .map_err(BridgeError::from).and_then(|(_, plan)| self.run_vault(&handle, &vault, plan, &spool));
        self.drop_spool(&spool);
        result
    }
    /// Delete a file, or an empty folder.
    pub fn vault_delete(&self, handle: String, path: Vec<String>, name: String, folder: bool) -> Result<()> {
        let vault = self.vault(&handle)?;
        let parent = vault.resolve(&self.core, &path)?;
        let plan = if folder { vault.plan_delete_folder(&self.core, &parent, &name)? } else { vault.plan_delete_file(&self.core, &parent, &name)? };
        let spool = Self::spool_path()?;
        self.run_vault(&handle, &vault, plan, &spool)
    }
    /// Move a file or folder (with its contents) from `path` into the folder
    /// at `target_path`, keeping its name (B12). Never replaces an existing
    /// entry; a folder cannot go into its own subtree.
    pub fn vault_move(&self, handle: String, path: Vec<String>, name: String, folder: bool, target_path: Vec<String>) -> Result<()> {
        let vault = self.vault(&handle)?;
        let parent = vault.resolve(&self.core, &path)?;
        let target = vault.resolve(&self.core, &target_path)?;
        if parent == target { return Err(BridgeError::Invalid); }
        let spool = Self::spool_path()?;
        let kind = if folder { EntryKind::Folder } else { EntryKind::File };
        let result = vault.prepare_rename(&self.core, EntryRef { folder: &parent, name: &name, kind },
            NewPlace { folder: &target, name: &name, modified_ms: now_ms() }, Spool { storage: self.app.as_ref(), path: &spool }, &mut |_| Ok(()))
            .map_err(BridgeError::from).and_then(|plan| self.run_vault(&handle, &vault, plan, &spool));
        self.drop_spool(&spool);
        result
    }
    /// Replace the content of file `name` at `path` with `source` (a new
    /// version; the vault name stays). Only if the stored file is still the
    /// version seen when starting, and `source` did not change while read (B12).
    pub fn vault_replace(&self, handle: String, path: Vec<String>, name: String, source: NativeLocation) -> Result<()> {
        let vault = self.vault(&handle)?;
        let folder = vault.resolve(&self.core, &path)?;
        let expected = vault.stat(&self.core, &folder, &name, EntryKind::File)?;
        let source = source.core()?;
        let mut input = self.core.open_read(&source)?;
        let meta = input.metadata()?;
        if meta.kind != paranoic_core::EntryKind::File { return Err(BridgeError::Unsupported); }
        let spool = Self::spool_path()?;
        let prepared = vault.prepare_file(&folder, NewFile { name: &name, modified_ms: meta.modified_unix_ms.unwrap_or_else(now_ms), size: meta.bytes },
            &mut input, Spool { storage: self.app.as_ref(), path: &spool }, &mut |_| Ok(()));
        let result = prepared.map_err(BridgeError::from).and_then(|p| {
            if input.metadata()? != meta || self.core.metadata(&source)? != meta { return Err(BridgeError::Changed); }
            self.run_vault(&handle, &vault, vault.plan_replace(&p, expected), &spool)
        });
        self.drop_spool(&spool);
        result
    }
    pub fn vault_rename(&self, handle: String, path: Vec<String>, name: String, new_name: String, folder: bool) -> Result<()> {
        let vault = self.vault(&handle)?;
        let parent = vault.resolve(&self.core, &path)?;
        let spool = Self::spool_path()?;
        let kind = if folder { EntryKind::Folder } else { EntryKind::File };
        let result = vault.prepare_rename(&self.core, EntryRef { folder: &parent, name: &name, kind },
            NewPlace { folder: &parent, name: &new_name, modified_ms: now_ms() }, Spool { storage: self.app.as_ref(), path: &spool }, &mut |_| Ok(()))
            .map_err(BridgeError::from).and_then(|plan| self.run_vault(&handle, &vault, plan, &spool));
        self.drop_spool(&spool);
        result
    }
}

pub(crate) fn run_wait(core: &paranoic_core::Core, id: paranoic_core::JobId) -> Result<Vec<paranoic_core::JobRecord>> {
    let until = std::time::Instant::now() + PLAN_TIMEOUT;
    loop {
        let job = core.job(id)?;
        if !matches!(job.state, JobState::Queued | JobState::Running) { return Ok(vec![job]); }
        if std::time::Instant::now() > until { return Err(BridgeError::Attention); }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Plaintext goes only to a staged object in the app's private workspace and
/// is published there only after the vault verified the whole file.
pub(crate) struct StageSink(pub(crate) Option<Box<dyn paranoic_core::StagedWrite>>);
impl std::io::Write for StageSink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> { self.0.as_mut().ok_or_else(|| std::io::Error::other("closed"))?.write(data) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
impl paranoic_crypto::StreamSink for StageSink {
    fn commit(mut self: Box<Self>) -> paranoic_crypto::Result<()> {
        self.0.take().ok_or(paranoic_crypto::CryptoError::Io)?.publish().map_err(|_| paranoic_crypto::CryptoError::Io)
    }
    /// A failed abort is reported, not hidden: the caller must not assume the
    /// staged plaintext is gone (external audit, 2026-11-14).
    fn abort(mut self: Box<Self>) -> paranoic_crypto::Result<()> {
        match self.0.take() {
            Some(stage) => stage.abort().map_err(|_| paranoic_crypto::CryptoError::CleanupFailed),
            None => Ok(()),
        }
    }
}

