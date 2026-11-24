//! Protected notes (roadmap 1.9): an isolated, encrypted, device-local notebook
//! with a fast Clear. Not synchronised, not reachable from the file manager,
//! cloud or search.
//!
//! Key hierarchy:
//! * a random 256-bit notes key encrypts the whole notebook (XChaCha20-Poly1305,
//!   context-bound, replaced atomically on save);
//! * the notes key is wrapped by the notes password (Argon2id, 64 MiB, 3 passes);
//! * when the host provides a device key (Android Keystore; on Windows only
//!   after the owner's decision on platform calls), that wrapped record is
//!   sealed once more with it. Clear destroys the device key first, so the
//!   record cannot be opened again even with the password;
//! * optional quick unlock (biometrics): a random secret sealed by a separate
//!   host key that requires user authentication wraps the same notes key.
//!   Off by default; the password remains the primary and stronger way in.
//!
//! Erasure boundary: Clear makes the content unreadable for the application by
//! destroying key material, then removes the encrypted objects. Flash storage
//! can keep old blocks; without a device key (or with one that is not rollback
//! resistant) an attacker holding an older disk image and the password could
//! still recover it. `ErasureGuarantee` states which case applies so the UI can
//! say so honestly. Exported or copied text is outside the application's reach.
use std::{path::Path, sync::{Arc, Mutex}, time::{Duration, SystemTime, UNIX_EPOCH}};
use paranoic_crypto::{CryptoError, ObjectId, PrivateDirectory, SecretKey};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

mod strength;
pub use strength::{password_feedback, password_strength, PasswordHint, PasswordStrength};
mod plain;
pub use plain::{export_plain, import_plain, PlainExport, PlainFile, PlainImport};

#[derive(Debug, thiserror::Error)]
pub enum NotesError {
    #[error("password attempts temporarily limited; retry after {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("wrong password")]
    WrongPassword,
    #[error("password does not meet the requirements")]
    WeakPassword,
    #[error("notes are not set up")]
    NotInitialized,
    #[error("notes are already set up")]
    AlreadyInitialized,
    #[error("notes are locked")]
    Locked,
    #[error("quick unlock is not enabled")]
    QuickUnlockDisabled,
    #[error("notebook exceeds the configured limits")]
    Limit,
    #[error("stored notes are damaged or were modified")]
    Corrupt,
    #[error("device key unavailable")]
    Device,
    #[error("storage operation failed")]
    Io,
    #[error("clear did not finish; some data or recovery keys may remain")]
    ClearIncomplete,
    #[error("too many wrong passwords: notes were erased")]
    Wiped,
    #[error("the duress password must differ from the notes password")]
    SamePassword,
}
pub type Result<T> = std::result::Result<T, NotesError>;

/// What Clear can promise on this device (shown in the UI).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErasureGuarantee {
    /// Device key with rollback resistance / secure element: destroying it is final.
    HardwareKey,
    /// Device key in the platform keystore without a rollback-resistance guarantee.
    DeviceKey,
    /// No device key: only the password protects leftover copies on flash.
    PasswordOnly,
}

/// Host key service (Android Keystore; Windows once decided). Keys are
/// non-exportable; `open` may require user authentication for the quick-unlock key.
pub trait DeviceKey: Send + Sync {
    /// Errors should be `NotesError::Device`.
    fn seal(&self, data: &[u8]) -> Result<Vec<u8>>;
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>>;
    /// Irrecoverably delete this key in the platform keystore.
    fn destroy(&self) -> Result<()>;
    fn guarantee(&self) -> ErasureGuarantee;
}

/// A file kept inside the notes (owner decision Р-3): stored as its own object
/// encrypted with the notes key, viewable inside the notes, never exported.
/// `id`: 32 hex digits of the object; `size`: plaintext bytes.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment { pub id: String, pub name: String, pub size: u64 }
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note { pub id: String, pub title: String, pub body: String, pub modified_ms: u64,
    #[serde(default)]
    pub attachments: Vec<Attachment> }
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tab { pub id: String, pub title: String, pub notes: Vec<Note> }
/// The whole notebook. Strings are wiped on drop (serde/allocator copies are not).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Notebook { pub tabs: Vec<Tab> }
impl Drop for Notebook {
    fn drop(&mut self) {
        for tab in &mut self.tabs {
            tab.title.zeroize(); tab.id.zeroize();
            for note in &mut tab.notes {
                note.title.zeroize(); note.body.zeroize(); note.id.zeroize();
                for attachment in &mut note.attachments { attachment.name.zeroize(); attachment.id.zeroize(); }
            }
        }
    }
}
impl Notebook {
    pub fn new_id() -> String { uuid::Uuid::new_v4().to_string() }
}

#[derive(Clone, Copy, Debug)]
pub struct NotesLimits { pub notebook_bytes: usize, pub tabs: usize, pub notes_per_tab: usize, pub title_chars: usize,
    /// One attachment (plaintext bytes); below the 16 MiB sealed-object limit.
    pub attachment_bytes: u64,
    /// All attachments together, including ones added but not yet saved.
    pub attachments_total_bytes: u64,
    pub attachments_per_note: usize }
impl Default for NotesLimits {
    fn default() -> Self { Self { notebook_bytes: 8 << 20, tabs: 256, notes_per_tab: 4096, title_chars: 256,
        attachment_bytes: 15 << 20, attachments_total_bytes: 256 << 20, attachments_per_note: 32 } }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotesState { NotSetUp, Locked }

const KEY: [u8; 16] = id(1);
const NOTEBOOK: [u8; 16] = id(2);
const ATTEMPTS: [u8; 16] = id(3);
const QUICK_SEALED: [u8; 16] = id(4);
const QUICK_WRAP: [u8; 16] = id(5);
const SETTINGS: [u8; 16] = id(6);
/// The notebook under a new key, written before the key switch (password change).
const NOTEBOOK_NEXT: [u8; 16] = id(7);
/// Objects of the store itself; every other object is an attachment.
/// Sealed with the notes key (`Extra`): the personal folder's secret (plan
/// 5.1) and whether a duress password is set (5.2). A password change
/// writes the new-key copy first, like the notebook.
const SAFE: [u8; 16] = id(8);
const SAFE_NEXT: [u8; 16] = id(9);
const CTX_SAFE: &[u8] = b"paranoic/notes/v1/extra";
/// Duress record (plan 5.2): same layout as the key record, a random key
/// wrapped with the duress password — or with a random password when none
/// is set, so the files do not show whether one exists.
const DURESS: [u8; 16] = id(10);
const CTX_DURESS: &[u8] = b"paranoic/notes/v1/duress-wrap";
const FIXED: [[u8; 16]; 10] = [KEY, NOTEBOOK, ATTEMPTS, QUICK_SEALED, QUICK_WRAP, SETTINGS, NOTEBOOK_NEXT, SAFE, SAFE_NEXT, DURESS];

#[derive(Clone, Default, Serialize, Deserialize)]
struct Extra { #[serde(default)] safe: Option<String>, #[serde(default)] duress: bool }
impl Drop for Extra { fn drop(&mut self) { if let Some(s) = &mut self.safe { s.zeroize(); } } }
const CTX_ATTACHMENT: &[u8] = b"paranoic/notes/v1/attachment/";
/// Wrong passwords in a row that trigger Clear when auto-wipe is enabled.
pub const AUTO_WIPE_ATTEMPTS: u32 = 10;
const fn id(n: u8) -> [u8; 16] { let mut b = [0u8; 16]; b[15] = n; b }
const KEY_MAGIC: &[u8; 8] = b"PNOTEK01";
const CTX_PASSWORD: &[u8] = b"paranoic/notes/v1/password-wrap";
const CTX_QUICK: &[u8] = b"paranoic/notes/v1/quick-wrap";
const CTX_NOTEBOOK: &[u8] = b"paranoic/notes/v1/notebook";
const MAX_RECORD: u64 = 4096;

enum Unlocked { Key(SecretKey), Duress }

struct Inner { generation: u64,
    /// Attachments written in this session but not yet referenced by a saved
    /// notebook (id → bytes): kept by the garbage collection until saved;
    /// dropped on lock, so unsaved attachments are removed later.
    pending: std::collections::HashMap<[u8; 16], u64> }

/// Owner of one notes directory (exclusive). Open once per process.
pub struct NotesStore {
    directory: PrivateDirectory,
    device: Option<Arc<dyn DeviceKey>>,
    /// Host key of quick unlock (biometrics), if the platform has one. Held by
    /// the store so that every Clear — including auto-wipe — destroys it.
    quick: Option<Arc<dyn DeviceKey>>,
    limits: NotesLimits,
    inner: Arc<Mutex<Inner>>,
}

/// Unlocked notes. Holds the notes key; invalidated by `lock`/Clear on the store.
pub struct NotesSession { key: SecretKey, generation: u64, inner: Arc<Mutex<Inner>> }

impl NotesStore {
    /// `directory`: app-private, excluded from backup, 0700 on Unix, dedicated to notes.
    pub fn open(directory: impl AsRef<Path>, device: Option<Arc<dyn DeviceKey>>, limits: NotesLimits) -> Result<Self> {
        let directory = PrivateDirectory::open(directory).map_err(io)?;
        Ok(Self { directory, device, quick: None, limits, inner: Arc::new(Mutex::new(Inner { generation: 0, pending: Default::default() })) })
    }
    /// Register the platform's quick-unlock key (before any other call).
    /// Clear and auto-wipe then destroy it as well (register У-27).
    pub fn with_quick_key(mut self, quick: Option<Arc<dyn DeviceKey>>) -> Self { self.quick = quick; self }
    pub fn state(&self) -> Result<NotesState> {
        Ok(if self.directory.contains(ObjectId::from_bytes(KEY)).map_err(io)? { NotesState::Locked } else { NotesState::NotSetUp })
    }
    pub fn guarantee(&self) -> ErasureGuarantee {
        self.device.as_ref().map_or(ErasureGuarantee::PasswordOnly, |d| d.guarantee())
    }
    pub fn quick_unlock_enabled(&self) -> Result<bool> {
        self.directory.contains(ObjectId::from_bytes(QUICK_WRAP)).map_err(io)
    }

    /// First setup (or after Clear): new notes key, empty notebook.
    pub fn create(&self, password: &str) -> Result<NotesSession> {
        if self.state()? != NotesState::NotSetUp { return Err(NotesError::AlreadyInitialized); }
        if password_strength(password) == PasswordStrength::Weak { return Err(NotesError::WeakPassword); }
        let key = SecretKey::generate().map_err(crypto)?;
        self.write_notebook(&key, &Notebook::default())?;
        self.write_key_record(&key, password)?;
        self.write_duress(None)?;
        self.session(key)
    }

    /// Password unlock with a persisted, growing delay after wrong passwords.
    pub fn unlock(&self, password: &str) -> Result<NotesSession> { Ok(self.unlock_detailed(password)?.0) }
    /// As `unlock`; `true` when the duress password was entered (plan 5.2):
    /// everything was erased (Clear) and new empty notes were set up with
    /// that password, so the session looks like a normal one. The host then
    /// removes the personal folder's files without saying anything.
    pub fn unlock_detailed(&self, password: &str) -> Result<(NotesSession, bool)> {
        match self.authenticate(password)? {
            Unlocked::Key(key) => Ok((self.session(key)?, false)),
            Unlocked::Duress => {
                self.clear(None)?;
                let key = SecretKey::generate().map_err(crypto)?;
                self.write_notebook(&key, &Notebook::default())?;
                self.write_key_record(&key, password)?;
                self.write_duress(None)?;
                Ok((self.session(key)?, true))
            }
        }
    }
    /// Set (`Some`) or remove (`None`) the duress password. It must be strong
    /// and differ from the notes password.
    pub fn set_duress(&self, session: &NotesSession, duress: Option<&str>) -> Result<()> {
        self.check(session)?;
        if let Some(password) = duress {
            if password_strength(password) == PasswordStrength::Weak { return Err(NotesError::WeakPassword); }
            let wrapped = self.open_key_record(&self.read(KEY)?)?;
            if SecretKey::unwrap(&wrapped, password.as_bytes(), CTX_PASSWORD).is_ok() { return Err(NotesError::SamePassword); }
        }
        self.write_duress(duress)?;
        let mut extra = self.open_extra(&session.key)?.unwrap_or_default();
        extra.duress = duress.is_some();
        self.write_extra(SAFE, &session.key, &extra)
    }
    pub fn duress_enabled(&self, session: &NotesSession) -> Result<bool> {
        self.check(session)?;
        Ok(self.open_extra(&session.key)?.is_some_and(|e| e.duress))
    }
    fn write_duress(&self, password: Option<&str>) -> Result<()> {
        let mut filler = Zeroizing::new(vec![0u8; 32]);
        paranoic_crypto::random_bytes(&mut filler).map_err(crypto)?;
        let throwaway = SecretKey::generate().map_err(crypto)?;
        let wrapped = throwaway.wrap(password.map_or(&filler[..], str::as_bytes), CTX_DURESS).map_err(crypto)?;
        self.write_wrapped(DURESS, &wrapped)
    }

    /// Quick unlock with the host's authentication-bound key (biometrics).
    pub fn unlock_quick(&self, quick: &dyn DeviceKey) -> Result<NotesSession> {
        if !self.quick_unlock_enabled()? { return Err(NotesError::QuickUnlockDisabled); }
        let sealed = self.read(QUICK_SEALED)?;
        let secret = Zeroizing::new(quick.open(&sealed).map_err(|_| NotesError::Device)?);
        let wrapped = self.read(QUICK_WRAP)?;
        let key = SecretKey::unwrap(&wrapped, &secret, CTX_QUICK).map_err(|_| NotesError::Corrupt)?;
        self.session(key)
    }

    pub fn enable_quick_unlock(&self, session: &NotesSession, quick: &dyn DeviceKey) -> Result<()> {
        self.check(session)?;
        // 256-bit random secret (two 128-bit OS-random draws), used as the
        // wrapping input; only its sealed form is stored.
        let mut raw = Zeroizing::new(vec![0u8; 32]);
        raw[..16].copy_from_slice(ObjectId::generate().map_err(crypto)?.as_bytes());
        raw[16..].copy_from_slice(ObjectId::generate().map_err(crypto)?.as_bytes());
        let sealed = quick.seal(&raw).map_err(|_| NotesError::Device)?;
        let wrap = session.key.wrap(&raw, CTX_QUICK).map_err(crypto)?;
        self.replace(QUICK_WRAP, &wrap)?;
        self.replace(QUICK_SEALED, &sealed)
    }
    pub fn disable_quick_unlock(&self) -> Result<()> {
        for object in [QUICK_SEALED, QUICK_WRAP] {
            if self.directory.contains(ObjectId::from_bytes(object)).map_err(io)? {
                self.directory.remove_object(ObjectId::from_bytes(object)).map_err(io)?;
            }
        }
        Ok(())
    }

    /// New password AND a new notes key (owner decision Р-1, register У-2): an
    /// old copy of the key record (backup, export) together with the old
    /// password no longer opens the current notes. Crash-safe order:
    /// 1. the notebook under the new key goes to a separate object;
    /// 2. quick unlock is turned off (its wrapper holds the old key; the user
    ///    enables it again);
    /// 3. the key record is replaced — the switch point;
    /// 4. the notebook object is replaced, the separate copy removed.
    ///
    /// A crash between 3 and 4 is finished by the next read (`open_notebook`).
    /// Returns the new session. Every older session is invalidated as soon as
    /// the new copy exists (before 2), also when a later step fails.
    pub fn change_password(&self, session: &NotesSession, old: &str, new: &str) -> Result<NotesSession> {
        self.check(session)?;
        if password_strength(new) == PasswordStrength::Weak { return Err(NotesError::WeakPassword); }
        if self.is_duress(new)? { return Err(NotesError::SamePassword); }
        let verified = match self.authenticate(old)? { Unlocked::Key(key) => key, Unlocked::Duress => return Err(NotesError::WrongPassword) };
        if verified.seal(b"x", b"probe").ok().and_then(|e| session.key.open(&e, b"probe").ok()).is_none() {
            return Err(NotesError::Corrupt);
        }
        let mut notebook = self.open_notebook(&session.key)?;
        let key = SecretKey::generate().map_err(crypto)?;
        // Attachments too: re-encrypted under the new key as NEW objects; the old
        // ones (old key) are removed by the collection below. Attachments added
        // but not saved yet are not part of the notebook and are dropped.
        for tab in &mut notebook.tabs {
            for note in &mut tab.notes {
                for attachment in &mut note.attachments {
                    let data = self.open_attachment(&session.key, &attachment.id)?;
                    attachment.id = self.write_attachment(&key, &data)?.hex();
                }
            }
        }
        self.write_sealed(NOTEBOOK_NEXT, &key, &notebook)?;
        let extra = self.open_extra(&session.key)?;
        if let Some(extra) = &extra { self.write_extra(SAFE_NEXT, &key, extra)?; }
        // End every old session BEFORE the switch. If the key record below is
        // replaced but reports an error (e.g. a failed directory sync), the
        // caller must not keep reading through the old key: such a read opens
        // the old notebook, removes the new copy and its attachments, and the
        // notes would be left under a key no record holds (external audit,
        // A-2). The next unlock decides by the record: old password — old
        // notebook; new password — the copy is finished (`open_notebook`).
        self.lock()?;
        self.disable_quick_unlock()?;
        self.write_key_record(&key, new)?;
        let fresh = self.session(key)?;
        self.write_notebook(&fresh.key, &notebook)?;
        let _ = self.directory.remove_object(ObjectId::from_bytes(NOTEBOOK_NEXT));
        if let Some(extra) = &extra {
            self.write_extra(SAFE, &fresh.key, extra)?;
            let _ = self.directory.remove_object(ObjectId::from_bytes(SAFE_NEXT));
        }
        // Old-key copies of the attachments must not stay readable (Р-1).
        self.collect_garbage(&notebook);
        Ok(fresh)
    }

    /// Plan 5.1: the 32-byte secret that opens the personal folder; with
    /// `create`, made on first use. One notes password opens both; Clear
    /// destroys it with everything else, so the folder can no longer be read.
    pub fn safe_secret(&self, session: &NotesSession, create: bool) -> Result<Option<Zeroizing<[u8; 32]>>> {
        self.check(session)?;
        let mut extra = self.open_extra(&session.key)?.unwrap_or_default();
        if extra.safe.is_none() {
            if !create { return Ok(None); }
            let mut fresh = Zeroizing::new([0u8; 32]);
            paranoic_crypto::random_bytes(fresh.as_mut()).map_err(crypto)?;
            extra.safe = Some(fresh.iter().map(|b| format!("{b:02x}")).collect());
            self.write_extra(SAFE, &session.key, &extra)?;
        }
        let hex = extra.safe.as_deref().ok_or(NotesError::Corrupt)?;
        if hex.len() != 64 { return Err(NotesError::Corrupt); }
        let mut secret = Zeroizing::new([0u8; 32]);
        for (i, byte) in secret.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| NotesError::Corrupt)?;
        }
        Ok(Some(secret))
    }
    fn write_extra(&self, object: [u8; 16], key: &SecretKey, extra: &Extra) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(extra).map_err(|_| NotesError::Corrupt)?);
        let sealed = key.seal(&plain, CTX_SAFE).map_err(crypto)?;
        self.replace(object, &sealed)
    }
    fn read_extra(&self, object: [u8; 16], key: &SecretKey) -> Result<Extra> {
        let sealed = self.read_bounded(object, 1024)?;
        let plain = Zeroizing::new(key.open(&sealed, CTX_SAFE).map_err(|_| NotesError::Corrupt)?);
        serde_json::from_slice(&plain).map_err(|_| NotesError::Corrupt)
    }
    /// Like `open_notebook`: a password change stopped after its key switch
    /// is finished from the new-key copy; a leftover copy is removed.
    fn open_extra(&self, key: &SecretKey) -> Result<Option<Extra>> {
        let (main, next) = (ObjectId::from_bytes(SAFE), ObjectId::from_bytes(SAFE_NEXT));
        let has_next = self.directory.contains(next).map_err(io)?;
        if !self.directory.contains(main).map_err(io)? {
            if !has_next { return Ok(None); }
        } else {
            match self.read_extra(SAFE, key) {
                Ok(extra) => { if has_next { let _ = self.directory.remove_object(next); } return Ok(Some(extra)); }
                Err(NotesError::Corrupt) if has_next => {}
                Err(error) => return Err(error),
            }
        }
        let extra = self.read_extra(SAFE_NEXT, key)?;
        self.write_extra(SAFE, key, &extra)?;
        let _ = self.directory.remove_object(next);
        Ok(Some(extra))
    }

    pub fn read_notebook(&self, session: &NotesSession) -> Result<Notebook> {
        self.check(session)?;
        let notebook = self.open_notebook(&session.key)?;
        // Leftovers of an interrupted save or password change are removed here.
        self.collect_garbage(&notebook);
        Ok(notebook)
    }

    /// Store a file as an attachment (encrypted with the notes key). Returns the
    /// reference to put into a note; it is kept until a saved notebook refers
    /// to it, or removed after the next lock if never saved.
    pub fn add_attachment(&self, session: &NotesSession, name: &str, data: &[u8]) -> Result<Attachment> {
        self.check(session)?;
        if name.trim().is_empty() || name.chars().count() > self.limits.title_chars || name.chars().any(char::is_control) {
            return Err(NotesError::Limit);
        }
        if data.len() as u64 > self.limits.attachment_bytes { return Err(NotesError::Limit); }
        let saved: u64 = self.open_notebook(&session.key)?.tabs.iter().flat_map(|t| &t.notes).flat_map(|n| &n.attachments).map(|a| a.size).sum();
        let pending: u64 = self.inner.lock().map_err(|_| NotesError::Io)?.pending.values().sum();
        if saved.saturating_add(pending).saturating_add(data.len() as u64) > self.limits.attachments_total_bytes { return Err(NotesError::Limit); }
        let id = self.write_attachment(&session.key, data)?;
        self.inner.lock().map_err(|_| NotesError::Io)?.pending.insert(*id.as_bytes(), data.len() as u64);
        Ok(Attachment { id: id.hex(), name: name.to_owned(), size: data.len() as u64 })
    }
    /// Plaintext of an attachment, for viewing inside the notes only.
    pub fn read_attachment(&self, session: &NotesSession, id: &str) -> Result<Zeroizing<Vec<u8>>> {
        self.check(session)?;
        self.open_attachment(&session.key, id)
    }
    fn attachment_context(id: &ObjectId) -> Vec<u8> { [CTX_ATTACHMENT, id.as_bytes().as_slice()].concat() }
    fn attachment_id(id: &str) -> Result<ObjectId> {
        let id = ObjectId::parse(id).ok_or(NotesError::Corrupt)?;
        if FIXED.contains(id.as_bytes()) { return Err(NotesError::Corrupt); }
        Ok(id)
    }
    fn write_attachment(&self, key: &SecretKey, data: &[u8]) -> Result<ObjectId> {
        let id = ObjectId::generate().map_err(crypto)?;
        let sealed = key.seal(data, &Self::attachment_context(&id)).map_err(crypto)?;
        self.replace(*id.as_bytes(), &sealed)?;
        Ok(id)
    }
    fn open_attachment(&self, key: &SecretKey, id: &str) -> Result<Zeroizing<Vec<u8>>> {
        let id = Self::attachment_id(id)?;
        let sealed = self.read_bounded(*id.as_bytes(), self.limits.attachment_bytes + 64)?;
        // The context binds the object id: an attachment copied under another id fails.
        key.open(&sealed, &Self::attachment_context(&id)).map_err(|_| NotesError::Corrupt)
    }
    /// Remove every object that is neither the store's own, nor referenced by
    /// `notebook`, nor added in this session and not yet saved. Best effort:
    /// Clear removes everything anyway.
    fn collect_garbage(&self, notebook: &Notebook) {
        let Ok(objects) = self.directory.list_objects() else { return };
        let mut keep: std::collections::HashSet<[u8; 16]> = FIXED.iter().copied().collect();
        for attachment in notebook.tabs.iter().flat_map(|t| &t.notes).flat_map(|n| &n.attachments) {
            if let Some(id) = ObjectId::parse(&attachment.id) { keep.insert(*id.as_bytes()); }
        }
        if let Ok(inner) = self.inner.lock() { keep.extend(inner.pending.keys().copied()); } else { return }
        for object in objects {
            if !keep.contains(object.as_bytes()) { let _ = self.directory.remove_object(object); }
        }
    }
    /// The notebook under `key`. If the main object does not open with it but
    /// the separate copy does, a password change stopped after its key switch:
    /// finish it (main object replaced, copy removed). A copy left by an
    /// interrupted or finished change is removed once the main object opens.
    fn open_notebook(&self, key: &SecretKey) -> Result<Notebook> {
        let next = ObjectId::from_bytes(NOTEBOOK_NEXT);
        match self.open_sealed(NOTEBOOK, key) {
            Ok(notebook) => {
                if self.directory.contains(next).map_err(io)? { let _ = self.directory.remove_object(next); }
                Ok(notebook)
            }
            Err(NotesError::Corrupt) if self.directory.contains(next).map_err(io)? => {
                let notebook = self.open_sealed(NOTEBOOK_NEXT, key)?;
                self.write_notebook(key, &notebook)?;
                let _ = self.directory.remove_object(next);
                Ok(notebook)
            }
            Err(error) => Err(error),
        }
    }
    fn open_sealed(&self, object: [u8; 16], key: &SecretKey) -> Result<Notebook> {
        let sealed = self.read_bounded(object, self.limits.notebook_bytes as u64 + 64)?;
        let plain = Zeroizing::new(key.open(&sealed, CTX_NOTEBOOK).map_err(|_| NotesError::Corrupt)?);
        let notebook = serde_json::from_slice(&plain).map_err(|_| NotesError::Corrupt)?;
        self.validate(&notebook)?;
        Ok(notebook)
    }

    /// Replace the whole notebook atomically (old or new version, never a mix).
    pub fn save_notebook(&self, session: &NotesSession, notebook: &Notebook) -> Result<()> {
        self.check(session)?;
        self.validate(notebook)?;
        // Every referenced attachment must exist (saved before, or added now).
        for attachment in notebook.tabs.iter().flat_map(|t| &t.notes).flat_map(|n| &n.attachments) {
            if !self.directory.contains(Self::attachment_id(&attachment.id)?).map_err(io)? { return Err(NotesError::Corrupt); }
        }
        self.write_notebook(&session.key, notebook)?;
        if let Ok(mut inner) = self.inner.lock() {
            for attachment in notebook.tabs.iter().flat_map(|t| &t.notes).flat_map(|n| &n.attachments) {
                if let Some(id) = ObjectId::parse(&attachment.id) { inner.pending.remove(id.as_bytes()); }
            }
        }
        // Attachments removed from notes are deleted here.
        self.collect_garbage(notebook);
        Ok(())
    }

    /// Invalidate every open session (Quick Lock, background, timeout).
    pub fn lock(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| NotesError::Io)?;
        inner.generation = inner.generation.wrapping_add(1);
        inner.pending.clear();
        Ok(())
    }

    /// Clear: sessions revoked, device keys destroyed, then every object
    /// removed. Afterwards the store is empty (`NotSetUp`). Returns the
    /// guarantee that applied; ClearIncomplete if something could not be removed.
    pub fn clear(&self, quick: Option<&dyn DeviceKey>) -> Result<ErasureGuarantee> {
        let quick = quick.or(self.quick.as_deref());
        self.lock()?;
        let guarantee = self.guarantee();
        // A quick-unlock wrapper is an independent recovery path. Without its
        // host key we can remove current files, but cannot revoke older copies.
        let mut complete = quick.is_some() || !self.quick_unlock_enabled()?;
        if let Some(device) = &self.device { complete &= device.destroy().is_ok(); }
        if let Some(quick) = quick { complete &= quick.destroy().is_ok(); }
        // The key record first: without it the notebook is unreadable.
        for object in [KEY, QUICK_WRAP, QUICK_SEALED] {
            match self.directory.contains(ObjectId::from_bytes(object)) {
                Ok(true) => complete &= self.directory.remove_object(ObjectId::from_bytes(object)).is_ok(),
                Ok(false) => {}
                Err(_) => complete = false,
            }
        }
        complete &= self.directory.clear_objects().is_ok();
        if complete && self.state()? == NotesState::NotSetUp { Ok(guarantee) } else { Err(NotesError::ClearIncomplete) }
    }

    /// Remaining delay before the next password attempt (for the UI).
    pub fn retry_after(&self) -> Result<Duration> {
        Ok(Duration::from_millis(self.attempts()?.next_ms.saturating_sub(now_ms())))
    }
    /// Wrong passwords in a row since the last successful unlock.
    pub fn failed_attempts(&self) -> Result<u32> { Ok(self.attempts()?.total) }

    /// Auto-wipe: Clear after `AUTO_WIPE_ATTEMPTS` wrong passwords in a row
    /// (quick unlock does not count). Off by default; changeable only while
    /// unlocked. Someone able to rewrite the app's files directly can reset the
    /// counter; against them only the encryption protects.
    pub fn auto_wipe_enabled(&self) -> Result<bool> {
        if !self.directory.contains(ObjectId::from_bytes(SETTINGS)).map_err(io)? { return Ok(false); }
        // Damaged settings are NOT consent to destroy the notes: auto-wipe is
        // only on when the user enabled it and the record says so (external
        // audit, 2026-11-14; rule 6).
        Ok(self.read(SETTINGS)?.first().is_some_and(|flag| *flag == 1))
    }
    pub fn set_auto_wipe(&self, session: &NotesSession, enabled: bool) -> Result<()> {
        self.check(session)?;
        self.replace(SETTINGS, &[u8::from(enabled)])
    }

    fn session(&self, key: SecretKey) -> Result<NotesSession> {
        let generation = self.inner.lock().map_err(|_| NotesError::Io)?.generation;
        Ok(NotesSession { key, generation, inner: Arc::clone(&self.inner) })
    }
    fn check(&self, session: &NotesSession) -> Result<()> {
        if !Arc::ptr_eq(&session.inner, &self.inner) { return Err(NotesError::Locked); }
        let current = self.inner.lock().map_err(|_| NotesError::Io)?.generation;
        if current != session.generation { return Err(NotesError::Locked); }
        if self.state()? != NotesState::Locked { return Err(NotesError::Locked); }
        Ok(())
    }
    /// True if `password` opens the duress record (one key derivation).
    fn is_duress(&self, password: &str) -> Result<bool> {
        let Ok(record) = self.read(DURESS) else { return Ok(false) };
        let wrapped = self.open_key_record(&record)?;
        Ok(SecretKey::unwrap(&wrapped, password.as_bytes(), CTX_DURESS).is_ok())
    }
    fn authenticate(&self, password: &str) -> Result<Unlocked> {
        if self.state()? != NotesState::Locked { return Err(NotesError::NotInitialized); }
        let attempts = self.attempts()?;
        let now = now_ms();
        if attempts.next_ms > now { return Err(NotesError::RateLimited { retry_after_ms: attempts.next_ms - now }); }
        let record = self.read(KEY)?;
        let wrapped = self.open_key_record(&record)?;
        let real = SecretKey::unwrap(&wrapped, password.as_bytes(), CTX_PASSWORD);
        // Always the second derivation too: the time does not tell which
        // password was entered, nor whether a duress password exists.
        let duress = self.is_duress(password)?;
        if !self.directory.contains(ObjectId::from_bytes(DURESS)).map_err(io)? && real.is_ok() { self.write_duress(None)?; }
        match real {
            Ok(key) => { if attempts.total != 0 || attempts.delay_level != 0 { self.store_attempts(Attempts::default())?; } Ok(Unlocked::Key(key)) }
            Err(CryptoError::Authentication) if duress => Ok(Unlocked::Duress),
            Err(CryptoError::Authentication) => {
                let delay_level = attempts.delay_level.saturating_add(1).min(6);
                let total = attempts.total.saturating_add(1);
                let delay = (1u64 << (delay_level - 1)).min(30) * 1000;
                // Persisted before reporting: a restart resets neither the delay nor the count.
                self.store_attempts(Attempts { delay_level, next_ms: now_ms().saturating_add(delay), total })?;
                if total >= AUTO_WIPE_ATTEMPTS && self.auto_wipe_enabled()? {
                    return match self.clear(None) { Ok(_) => Err(NotesError::Wiped), Err(error) => Err(error) };
                }
                Err(NotesError::WrongPassword)
            }
            Err(error) => Err(crypto(error)),
        }
    }
    fn write_key_record(&self, key: &SecretKey, password: &str) -> Result<()> {
        let wrapped = key.wrap(password.as_bytes(), CTX_PASSWORD).map_err(crypto)?;
        self.write_wrapped(KEY, &wrapped)
    }
    /// Key-record layout (magic, device flag, payload) for `object`.
    fn write_wrapped(&self, object: [u8; 16], wrapped: &[u8]) -> Result<()> {
        let wrapped = wrapped.to_vec();
        let (flag, payload) = match &self.device {
            Some(device) => (1u8, device.seal(&wrapped).map_err(|_| NotesError::Device)?),
            None => (0u8, wrapped),
        };
        let mut record = KEY_MAGIC.to_vec(); record.push(flag); record.extend_from_slice(&payload);
        self.replace(object, &record)
    }
    fn open_key_record(&self, record: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if record.len() < 9 || &record[..8] != KEY_MAGIC { return Err(NotesError::Corrupt); }
        match (record[8], &self.device) {
            (0, _) => Ok(Zeroizing::new(record[9..].to_vec())),
            // A device-sealed record never falls back to password-only.
            (1, Some(device)) => device.open(&record[9..]).map(Zeroizing::new).map_err(|_| NotesError::Device),
            (1, None) => Err(NotesError::Device),
            _ => Err(NotesError::Corrupt),
        }
    }
    fn write_notebook(&self, key: &SecretKey, notebook: &Notebook) -> Result<()> { self.write_sealed(NOTEBOOK, key, notebook) }
    fn write_sealed(&self, object: [u8; 16], key: &SecretKey, notebook: &Notebook) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(notebook).map_err(|_| NotesError::Corrupt)?);
        if plain.len() > self.limits.notebook_bytes { return Err(NotesError::Limit); }
        let sealed = key.seal(&plain, CTX_NOTEBOOK).map_err(crypto)?;
        self.replace(object, &sealed)
    }
    fn validate(&self, notebook: &Notebook) -> Result<()> {
        let l = self.limits;
        let long = |s: &str| s.chars().count() > l.title_chars;
        if notebook.tabs.len() > l.tabs { return Err(NotesError::Limit); }
        let mut attachment_ids = std::collections::HashSet::new();
        let mut total = 0u64;
        let mut tab_ids = std::collections::HashSet::new();
        for tab in &notebook.tabs {
            if tab.notes.len() > l.notes_per_tab || long(&tab.title) || tab.id.len() > 64 { return Err(NotesError::Limit); }
            if tab.notes.iter().any(|n| long(&n.title) || n.id.len() > 64) { return Err(NotesError::Limit); }
            for note in &tab.notes {
                if note.attachments.len() > l.attachments_per_note { return Err(NotesError::Limit); }
                for attachment in &note.attachments {
                    if long(&attachment.name) || attachment.size > l.attachment_bytes { return Err(NotesError::Limit); }
                    let id = Self::attachment_id(&attachment.id)?;
                    if !attachment_ids.insert(*id.as_bytes()) { return Err(NotesError::Corrupt); }
                    total = total.saturating_add(attachment.size);
                }
            }
            if tab.id.is_empty() || !tab_ids.insert(&tab.id) { return Err(NotesError::Corrupt); }
            let mut note_ids = std::collections::HashSet::new();
            if tab.notes.iter().any(|n| n.id.is_empty() || !note_ids.insert(&n.id)) { return Err(NotesError::Corrupt); }
        }
        if total > l.attachments_total_bytes { return Err(NotesError::Limit); }
        Ok(())
    }
    fn attempts(&self) -> Result<Attempts> {
        if !self.directory.contains(ObjectId::from_bytes(ATTEMPTS)).map_err(io)? { return Ok(Attempts::default()); }
        let data = self.read(ATTEMPTS)?;
        // Damaged record: maximum delay once, count as high as it can be. The
        // record is rewritten, so the delay runs out instead of restarting on
        // every read (which locked the notes out for good; external audit).
        if data.len() != 16 {
            let reset = Attempts { delay_level: 6, next_ms: now_ms().saturating_add(30_000), total: AUTO_WIPE_ATTEMPTS - 1 };
            self.store_attempts(reset)?;
            return Ok(reset);
        }
        let word = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let mut next = [0u8; 8]; next.copy_from_slice(&data[4..12]);
        // A clock moved backwards keeps at most the maximum delay.
        Ok(Attempts { delay_level: word(0).min(6), next_ms: u64::from_le_bytes(next).min(now_ms().saturating_add(30_000)), total: word(12) })
    }
    fn store_attempts(&self, attempts: Attempts) -> Result<()> {
        let mut data = attempts.delay_level.to_le_bytes().to_vec();
        data.extend_from_slice(&attempts.next_ms.to_le_bytes());
        data.extend_from_slice(&attempts.total.to_le_bytes());
        self.replace(ATTEMPTS, &data)
    }
    fn read(&self, object: [u8; 16]) -> Result<Vec<u8>> { self.read_bounded(object, MAX_RECORD) }
    fn read_bounded(&self, object: [u8; 16], limit: u64) -> Result<Vec<u8>> {
        use std::io::Read;
        let mut data = Vec::new();
        self.directory.open_object(ObjectId::from_bytes(object)).map_err(|_| NotesError::NotInitialized)?
            .take(limit + 1).read_to_end(&mut data).map_err(|_| NotesError::Io)?;
        if data.len() as u64 > limit { return Err(NotesError::Corrupt); }
        Ok(data)
    }
    fn replace(&self, object: [u8; 16], data: &[u8]) -> Result<()> {
        use std::io::Write;
        let mut sink = self.directory.stage_replace(ObjectId::from_bytes(object)).map_err(io)?;
        if sink.write_all(data).is_err() { let _ = sink.abort(); return Err(NotesError::Io); }
        sink.commit().map_err(io)
    }
}

#[derive(Clone, Copy, Default)]
struct Attempts { delay_level: u32, next_ms: u64, total: u32 }

fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }
fn crypto(error: CryptoError) -> NotesError {
    match error { CryptoError::LimitExceeded => NotesError::Limit, CryptoError::Authentication => NotesError::Corrupt, _ => NotesError::Io }
}
fn io(error: CryptoError) -> NotesError {
    match error { CryptoError::Busy => NotesError::Io, CryptoError::Permissions => NotesError::Device, _ => NotesError::Io }
}
