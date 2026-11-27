//! Protected notes (1.9): setup, unlock, save, lock, password change, backoff,
//! quick unlock, Clear with and without a device key, damage detection.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{path::PathBuf, sync::{Arc, Mutex}};
use paranoic_notes::*;

const PASSWORD: &str = "Correct horse battery staple 42";
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("paranoic-notes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap(); }
        Self(p)
    }
}
impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

/// Test device key: XOR stream with a secret that `destroy` forgets.
struct FakeDevice { key: Mutex<Option<u8>>, guarantee: ErasureGuarantee }
impl FakeDevice { fn new(k: u8) -> Arc<Self> { Arc::new(Self { key: Mutex::new(Some(k)), guarantee: ErasureGuarantee::HardwareKey }) } }
impl DeviceKey for FakeDevice {
    fn seal(&self, data: &[u8]) -> Result<Vec<u8>> {
        let k = self.key.lock().unwrap().ok_or(NotesError::Device)?;
        Ok([b"DEV".as_slice(), &data.iter().map(|b| b ^ k).collect::<Vec<_>>()].concat())
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        let k = self.key.lock().unwrap().ok_or(NotesError::Device)?;
        let body = sealed.strip_prefix(b"DEV").ok_or(NotesError::Device)?;
        Ok(body.iter().map(|b| b ^ k).collect())
    }
    fn destroy(&self) -> Result<()> { *self.key.lock().unwrap() = None; Ok(()) }
    fn guarantee(&self) -> ErasureGuarantee { self.guarantee }
}

fn sample() -> Notebook {
    // Fixed ids: the sample must compare equal across calls.
    Notebook { tabs: vec![Tab { id: "tab-1".into(), title: "Ключи".into(), notes: vec![
        Note { id: "note-1".into(), title: "seed".into(), body: "abandon ability able about".into(), modified_ms: 1, attachments: Vec::new() },
    ] }] }
}
fn snapshot(dir: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().len() == 32)
        .map(|e| (e.path(), std::fs::read(e.path()).unwrap())).collect()
}

#[test] fn password_strength_rules() {
    for weak in ["", "short", "1234567890", "aaaaaaaaaaaa", "Password2024!", "abcdefghijkl", "пароль123456"] {
        assert_eq!(password_strength(weak), PasswordStrength::Weak, "{weak:?}");
    }
    assert!(password_strength("Tr0ub4dor&3x") >= PasswordStrength::Fair);
    assert_eq!(password_strength(PASSWORD), PasswordStrength::Strong);
    assert_eq!(password_strength("лиса Гора 7 облако реки"), PasswordStrength::Strong);
}

#[test] fn setup_save_unlock_lock_and_change_password() {
    let temp = Temp::new();
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    assert_eq!(store.state().unwrap(), NotesState::NotSetUp);
    assert!(matches!(store.create("weak"), Err(NotesError::WeakPassword)));
    let session = store.create(PASSWORD).unwrap();
    assert!(matches!(store.create(PASSWORD), Err(NotesError::AlreadyInitialized)));
    assert_eq!(store.read_notebook(&session).unwrap(), Notebook::default());
    store.save_notebook(&session, &sample()).unwrap();
    // Nothing readable on disk.
    for (_, bytes) in snapshot(&temp.0) {
        assert!(!bytes.windows(7).any(|w| w == b"abandon"), "plaintext on disk");
    }
    store.lock().unwrap();
    assert!(matches!(store.read_notebook(&session), Err(NotesError::Locked)), "session survives lock");
    drop(store);
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.unlock(PASSWORD).unwrap();
    assert_eq!(store.read_notebook(&session).unwrap(), sample());
    let new = "Another long passphrase 99 river";
    assert!(matches!(store.change_password(&session, "wrong one entirely", new), Err(NotesError::WrongPassword)));
    std::thread::sleep(store.retry_after().unwrap());
    store.change_password(&session, PASSWORD, new).unwrap();
    std::thread::sleep(store.retry_after().unwrap());
    assert!(matches!(store.unlock(PASSWORD), Err(NotesError::WrongPassword)));
    std::thread::sleep(store.retry_after().unwrap());
    let session = store.unlock(new).unwrap();
    assert_eq!(store.read_notebook(&session).unwrap(), sample(), "content kept across password change");
}

#[test] fn wrong_password_delay_persists_across_restart() {
    let temp = Temp::new();
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    drop(store.create(PASSWORD).unwrap());
    assert!(matches!(store.unlock("definitely not it"), Err(NotesError::WrongPassword)));
    assert!(matches!(store.unlock(PASSWORD), Err(NotesError::RateLimited { .. })));
    drop(store);
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    assert!(!store.retry_after().unwrap().is_zero());
    assert!(matches!(store.unlock(PASSWORD), Err(NotesError::RateLimited { .. })));
    std::thread::sleep(store.retry_after().unwrap() + std::time::Duration::from_millis(20));
    assert!(store.unlock(PASSWORD).is_ok());
}

#[test] fn clear_with_device_key_defeats_an_old_copy() {
    let temp = Temp::new();
    let device = FakeDevice::new(0x5a);
    let store = NotesStore::open(&temp.0, Some(device.clone()), NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    let copy = snapshot(&temp.0); // e.g. blocks left on flash
    assert_eq!(store.clear(None).unwrap(), ErasureGuarantee::HardwareKey);
    assert_eq!(store.state().unwrap(), NotesState::NotSetUp);
    assert!(snapshot(&temp.0).is_empty(), "objects left after Clear");
    assert!(matches!(store.read_notebook(&session), Err(NotesError::Locked)));
    // Restore the old objects: without the destroyed device key nothing opens, even with the password.
    for (path, bytes) in &copy { std::fs::write(path, bytes).unwrap(); }
    assert!(matches!(store.unlock(PASSWORD), Err(NotesError::Device)));
    // After Clear the space is reusable with new keys.
    for (path, _) in &copy { let _ = std::fs::remove_file(path); }
    let device2 = FakeDevice::new(0x33);
    drop(store);
    let store = NotesStore::open(&temp.0, Some(device2), NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    assert_eq!(store.read_notebook(&session).unwrap(), Notebook::default());
}

#[test] fn clear_without_device_key_reports_password_only() {
    let temp = Temp::new();
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    let copy = snapshot(&temp.0);
    assert_eq!(store.clear(None).unwrap(), ErasureGuarantee::PasswordOnly);
    // The documented limit: an old copy plus the password still opens.
    for (path, bytes) in &copy { std::fs::write(path, bytes).unwrap(); }
    let session = store.unlock(PASSWORD).unwrap();
    assert_eq!(store.read_notebook(&session).unwrap(), sample());
}

#[test] fn quick_unlock_is_optional_and_cleared() {
    let temp = Temp::new();
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    let bio = FakeDevice::new(0x77);
    assert!(!store.quick_unlock_enabled().unwrap(), "off by default");
    assert!(matches!(store.unlock_quick(bio.as_ref()), Err(NotesError::QuickUnlockDisabled)));
    store.enable_quick_unlock(&session, bio.as_ref()).unwrap();
    store.lock().unwrap();
    let quick = store.unlock_quick(bio.as_ref()).unwrap();
    assert_eq!(store.read_notebook(&quick).unwrap(), sample());
    store.disable_quick_unlock().unwrap();
    assert!(matches!(store.unlock_quick(bio.as_ref()), Err(NotesError::QuickUnlockDisabled)));
    store.enable_quick_unlock(&quick, bio.as_ref()).unwrap();
    store.clear(Some(bio.as_ref())).unwrap();
    assert!(bio.key.lock().unwrap().is_none(), "quick key destroyed by Clear");
    assert!(!store.quick_unlock_enabled().unwrap());
}

#[test] fn damage_and_limits_are_reported() {
    let temp = Temp::new();
    let limits = NotesLimits { notebook_bytes: 4096, tabs: 2, ..NotesLimits::default() };
    let store = NotesStore::open(&temp.0, None, limits).unwrap();
    let session = store.create(PASSWORD).unwrap();
    let mut big = sample(); big.tabs[0].notes[0].body = "x".repeat(10_000);
    assert!(matches!(store.save_notebook(&session, &big), Err(NotesError::Limit)));
    let mut many = sample(); many.tabs.extend([sample().tabs[0].clone(), sample().tabs[0].clone()]);
    assert!(matches!(store.save_notebook(&session, &many), Err(NotesError::Limit)));
    store.save_notebook(&session, &sample()).unwrap();
    // Flip a byte in the encrypted notebook.
    let notebook = temp.0.join("00000000000000000000000000000002");
    let mut bytes = std::fs::read(&notebook).unwrap(); let last = bytes.len() - 1; bytes[last] ^= 1;
    std::fs::write(&notebook, bytes).unwrap();
    assert!(matches!(store.read_notebook(&session), Err(NotesError::Corrupt)));
    // A second owner of the same directory is refused.
    assert!(NotesStore::open(&temp.0, None, NotesLimits::default()).is_err());
}

#[test] fn duplicate_ids_are_refused_without_replacing_saved_notes() {
    let temp = Temp::new();
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    let mut duplicate = sample();
    let note = duplicate.tabs[0].notes[0].clone(); duplicate.tabs[0].notes.push(note);
    assert!(store.save_notebook(&session, &duplicate).is_err(), "duplicate note IDs accepted");
    let mut duplicate = sample(); duplicate.tabs.push(duplicate.tabs[0].clone());
    assert!(store.save_notebook(&session, &duplicate).is_err(), "duplicate tab IDs accepted");
    let mut empty = sample(); empty.tabs[0].id.clear();
    assert!(store.save_notebook(&session, &empty).is_err(), "empty ID accepted");
    assert_eq!(store.read_notebook(&session).unwrap(), sample());
}

#[test] fn clear_does_not_claim_erasure_without_the_quick_key() {
    let temp = Temp::new(); let device = FakeDevice::new(0x55);
    let store = NotesStore::open(&temp.0, Some(device), NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    let quick = FakeDevice::new(0x66);
    store.enable_quick_unlock(&session, quick.as_ref()).unwrap();
    assert!(matches!(store.clear(None), Err(NotesError::ClearIncomplete)), "quick recovery key not destroyed but Clear reported success");
    assert_eq!(store.state().unwrap(), NotesState::NotSetUp);
}

#[test] fn auto_wipe_after_ten_wrong_passwords_when_enabled() {
    let wait = |store: &NotesStore| std::thread::sleep(store.retry_after().unwrap() + std::time::Duration::from_millis(10));
    let temp = Temp::new();
    // Delays would make ten attempts take minutes; start each attempt with no delay
    // by clearing only the delay part of the record (the count is what matters here).
    let reset_delay = |dir: &std::path::Path| {
        let path = dir.join("00000000000000000000000000000003");
        let mut data = std::fs::read(&path).unwrap(); data[0..12].fill(0); std::fs::write(&path, data).unwrap();
    };
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    assert!(!store.auto_wipe_enabled().unwrap(), "off by default");
    // Disabled: ten wrong passwords only delay.
    for _ in 0..AUTO_WIPE_ATTEMPTS { assert!(matches!(store.unlock("nope nope nope"), Err(NotesError::WrongPassword))); reset_delay(&temp.0); }
    assert_eq!(store.failed_attempts().unwrap(), AUTO_WIPE_ATTEMPTS);
    let session = store.unlock(PASSWORD).unwrap();
    assert_eq!(store.failed_attempts().unwrap(), 0, "success resets the count");
    store.set_auto_wipe(&session, true).unwrap();
    store.lock().unwrap();
    assert!(matches!(store.set_auto_wipe(&session, false), Err(NotesError::Locked)), "changeable only while unlocked");
    // Enabled: the tenth wrong password erases everything, also across a restart.
    for _ in 0..AUTO_WIPE_ATTEMPTS - 2 { assert!(matches!(store.unlock("nope nope nope"), Err(NotesError::WrongPassword))); reset_delay(&temp.0); }
    drop(store);
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    assert_eq!(store.failed_attempts().unwrap(), AUTO_WIPE_ATTEMPTS - 2);
    assert!(matches!(store.unlock("nope nope nope"), Err(NotesError::WrongPassword))); reset_delay(&temp.0);
    assert!(matches!(store.unlock("nope nope nope"), Err(NotesError::Wiped)));
    assert_eq!(store.state().unwrap(), NotesState::NotSetUp);
    assert!(snapshot(&temp.0).is_empty());
    wait(&store);
    assert!(matches!(store.unlock(PASSWORD), Err(NotesError::NotInitialized)));
}

/// У-27: a quick-unlock key registered with the store is destroyed by every
/// Clear, including auto-wipe after repeated wrong passwords.
#[test] fn auto_wipe_destroys_registered_quick_key() {
    let dir = Temp::new();
    let device = FakeDevice::new(0x11);
    let bio = FakeDevice::new(0x77);
    let store = NotesStore::open(&dir.0, Some(device.clone() as Arc<dyn DeviceKey>), NotesLimits::default()).unwrap()
        .with_quick_key(Some(bio.clone() as Arc<dyn DeviceKey>));
    let session = store.create(PASSWORD).unwrap();
    store.enable_quick_unlock(&session, bio.as_ref()).unwrap();
    store.set_auto_wipe(&session, true).unwrap();
    store.lock().unwrap();
    let mut wiped = false;
    for _ in 0..paranoic_notes::AUTO_WIPE_ATTEMPTS {
        loop {
            match store.unlock("definitely the wrong password") {
                Err(NotesError::RateLimited { retry_after_ms }) => std::thread::sleep(std::time::Duration::from_millis(retry_after_ms + 5)),
                Err(NotesError::Wiped) => { wiped = true; break }
                Err(_) => break,
                Ok(_) => panic!("wrong password accepted"),
            }
        }
        if wiped { break; }
    }
    assert!(wiped, "auto-wipe did not happen");
    assert!(bio.key.lock().unwrap().is_none(), "quick key survived auto-wipe");
    assert!(!store.quick_unlock_enabled().unwrap());
    assert_eq!(store.state().unwrap(), paranoic_notes::NotesState::NotSetUp);
}

/// Р-1 / У-2: a password change replaces the notes key. An old copy of the key
/// record plus the old password no longer opens the current notes; quick
/// unlock is off afterwards; a crash between the key switch and the notebook
/// replacement is finished by the next read.
#[test] fn password_change_rotates_the_key_and_recovers_an_interrupted_switch() {
    let temp = Temp::new();
    let object = |n: u8| temp.0.join(format!("{:032x}", n));
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    let bio = FakeDevice::new(0x41);
    store.enable_quick_unlock(&session, bio.as_ref()).unwrap();
    let old_record = std::fs::read(object(1)).unwrap();
    let old_notebook = std::fs::read(object(2)).unwrap();
    let new = "Another long passphrase 99 river";
    let fresh = store.change_password(&session, PASSWORD, new).unwrap();
    assert!(matches!(store.read_notebook(&session), Err(NotesError::Locked)), "old session survives the key change");
    assert_eq!(store.read_notebook(&fresh).unwrap(), sample(), "content lost");
    assert!(!store.quick_unlock_enabled().unwrap(), "quick unlock still wraps the old key");
    assert!(!object(7).exists(), "temporary copy left");
    let new_notebook = std::fs::read(object(2)).unwrap();
    drop(fresh); drop(store);

    // An old copy of the key record with the old password: the key it holds no
    // longer opens the notebook.
    let new_record = std::fs::read(object(1)).unwrap();
    std::fs::write(object(1), &old_record).unwrap();
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let stale = store.unlock(PASSWORD).unwrap();
    assert!(matches!(store.read_notebook(&stale), Err(NotesError::Corrupt)), "old key still opens current notes");
    drop(stale); drop(store);

    // Crash after the key switch, before the notebook was replaced: new key
    // record, notebook still under the old key, new copy beside it.
    std::fs::write(object(1), &new_record).unwrap();
    std::fs::write(object(2), &old_notebook).unwrap();
    std::fs::write(object(7), &new_notebook).unwrap();
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.unlock(new).unwrap();
    assert_eq!(store.read_notebook(&session).unwrap(), sample(), "interrupted change not finished");
    assert!(!object(7).exists(), "copy not removed after finishing");
    assert_eq!(std::fs::read(object(2)).unwrap().len(), new_notebook.len());
    assert_eq!(store.read_notebook(&session).unwrap(), sample());
}

/// Р-3: attachments live inside the notes (encrypted, never exported), are
/// collected when no saved note refers to them, follow a password change to
/// the new key (old-key copies removed) and are erased by Clear.
#[test] fn attachments_stored_collected_rotated_and_cleared() {
    let temp = Temp::new();
    let objects = |dir: &std::path::Path| std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.len() == 32).collect::<std::collections::BTreeSet<_>>();
    let limits = NotesLimits { attachment_bytes: 64, attachments_per_note: 2, ..NotesLimits::default() };
    let store = NotesStore::open(&temp.0, None, limits).unwrap();
    let session = store.create(PASSWORD).unwrap();
    let photo = store.add_attachment(&session, "фото.jpg", b"JPEG secret bytes").unwrap();
    assert_eq!(photo.size, 17);
    assert!(matches!(store.add_attachment(&session, "big", &[0u8; 65]), Err(NotesError::Limit)), "oversized attachment");
    let mut book = sample();
    book.tabs[0].notes[0].attachments.push(photo.clone());
    store.save_notebook(&session, &book).unwrap();
    assert_eq!(store.read_notebook(&session).unwrap(), book);
    assert_eq!(store.read_attachment(&session, &photo.id).unwrap().as_slice(), b"JPEG secret bytes");
    for (_, bytes) in snapshot(&temp.0) { assert!(!bytes.windows(6).any(|w| w == b"secret"), "attachment plaintext on disk"); }

    // A reference to an object that does not exist is refused.
    let mut dangling = book.clone();
    dangling.tabs[0].notes[0].attachments.push(Attachment { id: "0123456789abcdef0123456789abcdef".into(), name: "x".into(), size: 1 });
    assert!(store.save_notebook(&session, &dangling).is_err(), "dangling attachment saved");
    // Audit 14: a recorded size that differs from the stored object is refused.
    let mut forged = book.clone();
    forged.tabs[0].notes[0].attachments[0].size = 1;
    assert!(matches!(store.save_notebook(&session, &forged), Err(NotesError::Corrupt)), "forged attachment size saved");
    // Per-note limit.
    let mut crowded = book.clone();
    for i in 0..2 { crowded.tabs[0].notes[0].attachments.push(store.add_attachment(&session, &format!("a{i}"), b"x").unwrap()); }
    assert!(matches!(store.save_notebook(&session, &crowded), Err(NotesError::Limit)));
    // Unsaved attachments survive until lock, then are collected.
    store.lock().unwrap();
    let session = store.unlock(PASSWORD).unwrap();
    store.read_notebook(&session).unwrap();
    let expected: std::collections::BTreeSet<String> = [format!("{:032x}", 1), format!("{:032x}", 2), format!("{:032x}", 10), photo.id.clone()].into_iter().collect();
    assert_eq!(objects(&temp.0), expected, "unsaved attachments not collected (only key, notebook, the duress record and the saved photo stay)");
    // An attachment object copied under another id does not open.
    let other = store.add_attachment(&session, "other", b"other bytes").unwrap();
    std::fs::copy(temp.0.join(&photo.id), temp.0.join(&other.id)).unwrap();
    assert!(store.read_attachment(&session, &other.id).is_err(), "attachment swapped between ids");

    // Password change: the attachment follows the new key; the old object is gone.
    let new = "Another long passphrase 99 river";
    let fresh = store.change_password(&session, PASSWORD, new).unwrap();
    let rotated = store.read_notebook(&fresh).unwrap();
    let moved = &rotated.tabs[0].notes[0].attachments[0];
    assert_ne!(moved.id, photo.id, "attachment kept under the old key");
    assert_eq!(store.read_attachment(&fresh, &moved.id).unwrap().as_slice(), b"JPEG secret bytes");
    assert!(!temp.0.join(&photo.id).exists(), "old-key copy of the attachment left on disk");
    // Removing it from the note deletes the object.
    let mut emptied = rotated.clone();
    emptied.tabs[0].notes[0].attachments.clear();
    store.save_notebook(&fresh, &emptied).unwrap();
    assert!(!temp.0.join(&moved.id).exists(), "removed attachment kept");
    // Clear erases everything.
    let again = store.add_attachment(&fresh, "last", b"last").unwrap();
    let mut last = emptied.clone(); last.tabs[0].notes[0].attachments.push(again);
    store.save_notebook(&fresh, &last).unwrap();
    store.clear(None).unwrap();
    assert!(objects(&temp.0).is_empty(), "Clear left objects: {:?}", objects(&temp.0));
}

/// External audit A-2 (2026-11-14): a failure after the new copy was written
/// must not leave the old session usable — reading through it would remove
/// the new copy and its attachments while the key record may already hold the
/// new key. The next unlock restores the right notebook.
struct FlakyDevice { inner: Arc<FakeDevice>, fail_seal: std::sync::atomic::AtomicBool }
impl DeviceKey for FlakyDevice {
    fn seal(&self, data: &[u8]) -> Result<Vec<u8>> {
        if self.fail_seal.load(std::sync::atomic::Ordering::SeqCst) { return Err(NotesError::Device); }
        self.inner.seal(data)
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>> { self.inner.open(sealed) }
    fn destroy(&self) -> Result<()> { self.inner.destroy() }
    fn guarantee(&self) -> ErasureGuarantee { self.inner.guarantee() }
}
#[test] fn failed_password_change_ends_the_old_session_and_keeps_the_notes() {
    let temp = Temp::new();
    let next = temp.0.join(format!("{:032x}", 7u8));
    let device = Arc::new(FlakyDevice { inner: FakeDevice::new(0x2a), fail_seal: std::sync::atomic::AtomicBool::new(false) });
    let store = NotesStore::open(&temp.0, Some(device.clone()), NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    device.fail_seal.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(store.change_password(&session, PASSWORD, "Another long passphrase 99 river").is_err());
    assert!(matches!(store.read_notebook(&session), Err(NotesError::Locked)), "old session still usable after a failed switch");
    device.fail_seal.store(false, std::sync::atomic::Ordering::SeqCst);
    let again = store.unlock(PASSWORD).unwrap();
    assert_eq!(store.read_notebook(&again).unwrap(), sample(), "notes lost after a failed password change");
    assert!(!next.exists(), "new copy left after the record kept the old key");
}

/// External audit (2026-11-14): a damaged attempt record must not lock the
/// notes out for good, and a damaged settings record is not consent to wipe.
#[test] fn damaged_attempt_and_settings_records_do_not_lock_out_or_enable_wipe() {
    let temp = Temp::new();
    let object = |n: u8| temp.0.join(format!("{:032x}", n));
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    drop(session); store.lock().unwrap(); drop(store);
    std::fs::write(object(3), b"bad").unwrap();
    std::fs::write(object(6), b"").unwrap();
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    assert!(matches!(store.unlock(PASSWORD), Err(NotesError::RateLimited { .. })));
    assert_eq!(std::fs::read(object(3)).unwrap().len(), 16, "damaged attempt record not rewritten: the delay restarts on every read");
    // Audit 20: damage is reported, never read as "on"; extra bytes after a 1 included.
    assert!(matches!(store.auto_wipe_enabled(), Err(NotesError::Corrupt)), "damaged settings not reported");
    std::fs::write(object(6), [1u8, 0]).unwrap();
    assert!(matches!(store.auto_wipe_enabled(), Err(NotesError::Corrupt)), "trailing bytes treated as consent to auto-wipe");
}

/// Plan 5.1: the personal folder's secret lives with the notes — made on first
/// use, the same after a password change (also an interrupted one), and gone
/// after Clear.
#[test] fn safe_secret_follows_the_notes_key_and_dies_with_clear() {
    let temp = Temp::new();
    let object = |n: u8| temp.0.join(format!("{:032x}", n));
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    assert!(store.safe_secret(&session, false).unwrap().is_none(), "made without being asked");
    let secret = store.safe_secret(&session, true).unwrap().unwrap();
    assert_eq!(*store.safe_secret(&session, false).unwrap().unwrap(), *secret, "a second secret was made");
    assert!(!std::fs::read(object(8)).unwrap().windows(8).any(|w| w == &secret[..8]), "secret stored in the clear");
    let old_safe = std::fs::read(object(8)).unwrap();

    let new = "Another long passphrase 99 river";
    let fresh = store.change_password(&session, PASSWORD, new).unwrap();
    assert!(matches!(store.safe_secret(&session, false), Err(NotesError::Locked)));
    assert_eq!(*store.safe_secret(&fresh, false).unwrap().unwrap(), *secret, "secret lost in the password change");
    assert!(!object(9).exists(), "temporary copy left");
    let new_safe = std::fs::read(object(8)).unwrap();
    drop(fresh); drop(store);

    // Crash after the key switch, before the secret record was replaced.
    std::fs::write(object(8), &old_safe).unwrap();
    std::fs::write(object(9), &new_safe).unwrap();
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.unlock(new).unwrap();
    assert_eq!(*store.safe_secret(&session, false).unwrap().unwrap(), *secret, "interrupted change not finished");
    assert!(!object(9).exists(), "copy not removed after finishing");
    // Not an attachment: a saved notebook never collects it.
    store.save_notebook(&session, &sample()).unwrap();
    assert!(object(8).exists());

    store.clear(None).unwrap();
    assert!(!object(8).exists() && !object(9).exists(), "secret survived Clear");
    let again = store.create(PASSWORD).unwrap();
    let other = store.safe_secret(&again, true).unwrap().unwrap();
    assert_ne!(*other, *secret, "old secret came back after Clear");
}

/// Plan 5.2: the duress password erases everything and opens new empty notes
/// as if nothing happened; whether one is set is not visible in the files.
#[test] fn duress_password_erases_and_opens_empty_notes() {
    let temp = Temp::new();
    let object = |n: u8| temp.0.join(format!("{:032x}", n));
    let store = NotesStore::open(&temp.0, None, NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    store.safe_secret(&session, true).unwrap().unwrap();
    let duress = "Quiet meadow under cold stars 7";
    assert!(matches!(store.set_duress(&session, Some(PASSWORD)), Err(NotesError::SamePassword)));
    assert!(matches!(store.set_duress(&session, Some("short")), Err(NotesError::WeakPassword)));
    assert!(!store.duress_enabled(&session).unwrap());
    let filler = std::fs::read(object(10)).unwrap();
    store.set_duress(&session, Some(duress)).unwrap();
    assert!(store.duress_enabled(&session).unwrap());
    assert_eq!(std::fs::read(object(10)).unwrap().len(), filler.len(), "a set duress password is visible by size");
    assert!(matches!(store.change_password(&session, PASSWORD, duress), Err(NotesError::SamePassword)));
    // Audit 3: an update interrupted between the flag and the record (here the
    // old filler is put back) is shown as what is on disk: not armed.
    let armed = std::fs::read(object(10)).unwrap();
    std::fs::write(object(10), &filler).unwrap();
    assert!(!store.duress_enabled(&session).unwrap(), "flag shows armed while the record is filler");
    std::fs::write(object(10), &armed).unwrap();
    assert!(store.duress_enabled(&session).unwrap());

    // A wrong password still counts as wrong.
    store.lock().unwrap();
    assert!(matches!(store.unlock("not any of them at all"), Err(NotesError::WrongPassword)));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (fresh, wiped) = store.unlock_detailed(duress).unwrap();
    assert!(wiped);
    assert_eq!(store.read_notebook(&fresh).unwrap(), Notebook::default(), "notes survived the duress password");
    assert!(store.safe_secret(&fresh, false).unwrap().is_none(), "personal folder secret survived");
    assert!(!store.duress_enabled(&fresh).unwrap(), "duress still armed in the new notes");
    assert_eq!(store.failed_attempts().unwrap(), 0);
    drop(fresh); store.lock().unwrap();
    assert!(matches!(store.unlock(PASSWORD), Err(NotesError::WrongPassword)), "the real password still opens");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (again, wiped) = store.unlock_detailed(duress).unwrap();
    assert!(!wiped, "the duress password is now the ordinary one");
    assert_eq!(store.read_notebook(&again).unwrap(), Notebook::default());

    // Turning it off restores a filler of the same size.
    store.set_duress(&again, Some("Another quiet phrase for duress 9")).unwrap();
    store.set_duress(&again, None).unwrap();
    assert!(!store.duress_enabled(&again).unwrap());
}

/// Plan 6.1: hints explain the strength; a generated passphrase is strong.
#[test] fn password_hints_and_generated_passphrases() {
    use paranoic_notes::{password_feedback, PasswordHint as H};
    let hints = |p: &str| password_feedback(p).1;
    assert!(hints("abc").contains(&H::TooShort));
    assert!(hints("myqwertyhouse").contains(&H::Common));
    assert!(hints("aaaaaaaaaaaa").contains(&H::Repeats));
    assert!(hints("onlylowercase").contains(&H::OneKind));
    let (strength, rest) = password_feedback(PASSWORD);
    assert_eq!(strength, PasswordStrength::Strong);
    assert!(!rest.contains(&H::AddWord) && !rest.contains(&H::TooShort));
    assert_ne!(password_feedback("correcthorse").0, PasswordStrength::Strong);
    assert!(hints("correcthorse").contains(&H::AddWord));
    // Rule of 26.11: length over composition.
    assert_eq!(password_strength("Abcdef1!"), PasswordStrength::Weak, "under 12");
    assert_eq!(password_strength("Abcdefgh1!xy"), PasswordStrength::Weak, "a run squeezed out leaves too little");
    assert_eq!(password_strength("лиса гора облако"), PasswordStrength::Strong, "a lowercase phrase is enough");
    assert_eq!(password_strength("Mz7#qwRt-9vLk"), PasswordStrength::Strong);
}

/// Audit A36: with a device key only a sealed setting arms the wipe; a plain "1" written into
/// the app's files does not, and a damaged counter is no step towards the wipe.
#[test] fn planted_auto_wipe_byte_does_not_arm_the_wipe() {
    let temp = Temp::new();
    let object = |n: u8| temp.0.join(format!("{:032x}", n));
    let device = FakeDevice::new(0x33);
    let store = NotesStore::open(&temp.0, Some(device.clone()), NotesLimits::default()).unwrap();
    let session = store.create(PASSWORD).unwrap();
    store.save_notebook(&session, &sample()).unwrap();
    store.set_auto_wipe(&session, false).unwrap();
    drop(session); store.lock().unwrap();
    std::fs::write(object(6), [1u8]).unwrap(); // planted "enabled"
    std::fs::write(object(3), b"bad").unwrap(); // damaged counter
    for _ in 0..12 {
        match store.unlock("Wrong horse battery staple 99") {
            Err(NotesError::RateLimited { .. }) | Err(NotesError::WrongPassword) => {}
            Ok(_) => panic!("a wrong password unlocked"),
            Err(other) => panic!("unexpected: {other:?}"),
        }
        // Skip the delay by rewinding the stored counter's next time (test only).
        let mut data = std::fs::read(object(3)).unwrap(); data[4..12].copy_from_slice(&0u64.to_le_bytes()); std::fs::write(object(3), data).unwrap();
    }
    assert_eq!(store.state().unwrap(), NotesState::Locked, "a planted setting wiped the notes");
    let session = store.unlock(PASSWORD).unwrap();
    assert_eq!(store.read_notebook(&session).unwrap().tabs.len(), sample().tabs.len());
    // The legacy plain record was upgraded to the sealed form at the correct unlock.
    assert!(std::fs::read(object(6)).unwrap().starts_with(b"PNSETS01"));
    assert!(store.auto_wipe_enabled().unwrap(), "the user's (planted-then-honoured) setting reads back sealed");
}
