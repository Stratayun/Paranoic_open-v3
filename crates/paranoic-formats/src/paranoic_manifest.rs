//! Version manifest of format 1.8 (spec docs/format/paranoic-v1.md §9, owner
//! decision Р-2). One sealed object `paranoic.manifest` per vault holds a
//! sequence number and, for every entry object, the hash of its current entry
//! header. A header identifies a whole file version (new random FK and file_id
//! per version, chunks bound to them), so presenting an older version, or an
//! object that was deleted, no longer matches. The sequence and a per-device
//! anchor of the highest sequence seen detect a whole-vault rollback.
//!
//! Records while a change is in flight keep the previous hash acceptable, so
//! an interrupted plan never looks like an attack; the next change settles
//! them against what the storage really holds.
use std::collections::BTreeMap;
use chacha20poly1305::{aead::{Aead, KeyInit, Payload}, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use paranoic_crypto::{CryptoError, Result};
use sha2::Sha256;
use crate::paranoic_v1::{RandomSource, VaultKeys, ENTRY_HEADER_LEN};

pub const MANIFEST_FILE: &str = "paranoic.manifest";
const MAGIC: &[u8; 8] = b"PRNMAN01";
const AD_TAG: &[u8] = b"paranoic/v1/manifest";
const RECORD_LEN: usize = 16 + 1 + 16 + 16;
const BODY_FIXED: usize = 8 + 4;
const TAG_LEN: usize = 16;
/// Upper bound of entries (≈ 49 MB sealed); larger manifests are refused.
pub const MAX_RECORDS: usize = 1_000_000;
const ZERO: [u8; 16] = [0u8; 16];

/// Identity of an entry object inside the manifest (keyed, so the manifest
/// does not reveal storage paths to whoever can read it unsealed).
pub type Locator = [u8; 16];
/// First 16 bytes of BLAKE3 of the 374-byte entry header.
pub type HeaderHash = [u8; 16];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordState {
    /// Settled: only `current` is accepted.
    Present = 0,
    /// A change is in flight: `current` (new) or `previous` (old, zero = the
    /// object did not exist before) is accepted.
    Changing = 1,
    /// A deletion is in flight: the object may still be there with `current`.
    Deleting = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record { pub state: RecordState, pub current: HeaderHash, pub previous: HeaderHash }

/// What the manifest says about an object found in the storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    /// No record: never written by a holder of the key, or deleted and settled.
    Unknown,
    /// A record exists, but for another version of the entry.
    Mismatch,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub sequence: u64,
    records: BTreeMap<Locator, Record>,
    /// Storage path (relative to the vault root) of every record in flight, so
    /// that `settle` can look at the object. Settled records keep no path.
    pending: BTreeMap<Locator, String>,
}
/// Longest object path kept for a record in flight ("d/aa/<30>/<26>.f" is 63).
const PATH_MAX: usize = 96;

impl Manifest {
    pub fn len(&self) -> usize { self.records.len() }
    pub fn is_empty(&self) -> bool { self.records.is_empty() }
    pub fn record(&self, locator: &Locator) -> Option<&Record> { self.records.get(locator) }
    /// Records still in flight (a change or deletion not yet settled).
    pub fn has_pending(&self) -> bool { !self.pending.is_empty() }
    pub fn records(&self) -> impl Iterator<Item = (&Locator, &Record)> { self.records.iter() }

    /// Is this header of the object at `locator` the version the manifest knows?
    pub fn check(&self, locator: &Locator, hash: &HeaderHash) -> Verdict {
        match self.records.get(locator) {
            None => Verdict::Unknown,
            Some(r) if r.current == *hash => Verdict::Accepted,
            Some(r) if r.state == RecordState::Changing && r.previous != ZERO && r.previous == *hash => Verdict::Accepted,
            Some(_) => Verdict::Mismatch,
        }
    }
    /// Initial state of an object that already exists (upgrade from plain v1).
    pub fn insert_present(&mut self, locator: Locator, hash: HeaderHash) -> Result<()> {
        if self.records.len() >= MAX_RECORDS && !self.records.contains_key(&locator) { return Err(CryptoError::LimitExceeded); }
        self.records.insert(locator, Record { state: RecordState::Present, current: hash, previous: ZERO });
        Ok(())
    }
    /// Before writing a new version (or a new object) at `locator` (`path`:
    /// its storage path relative to the vault root).
    pub fn begin_change(&mut self, locator: Locator, new_hash: HeaderHash, path: &str) -> Result<()> {
        if path.is_empty() || path.len() > PATH_MAX { return Err(CryptoError::InvalidInput); }
        let previous = match self.records.get(&locator) {
            Some(r) if r.state == RecordState::Changing => {
                // Another change was never settled: keep both acceptable only if
                // the storage settles it first.
                return Err(CryptoError::Busy);
            }
            Some(r) => r.current,
            None => {
                if self.records.len() >= MAX_RECORDS { return Err(CryptoError::LimitExceeded); }
                ZERO
            }
        };
        self.records.insert(locator, Record { state: RecordState::Changing, current: new_hash, previous });
        self.pending.insert(locator, path.to_owned());
        Ok(())
    }
    /// Before deleting the object at `locator` (`path` as for `begin_change`).
    pub fn begin_delete(&mut self, locator: Locator, path: &str) -> Result<()> {
        if path.is_empty() || path.len() > PATH_MAX { return Err(CryptoError::InvalidInput); }
        match self.records.get_mut(&locator) {
            Some(r) if r.state == RecordState::Present => { r.state = RecordState::Deleting; r.previous = ZERO; }
            Some(_) => return Err(CryptoError::Busy),
            None => return Err(CryptoError::InvalidInput),
        }
        self.pending.insert(locator, path.to_owned());
        Ok(())
    }
    /// Settle in-flight records against what the storage holds now
    /// (`observe(path)` = hash of the object's header, or None if absent).
    /// A change that did not happen falls back to the old version; a deletion
    /// that did not happen keeps the object.
    /// Storage paths (relative to the vault root) of records still in flight.
    pub fn pending_paths(&self) -> Vec<String> { self.pending.values().cloned().collect() }
    pub fn settle(&mut self, observe: impl FnMut(&str) -> Result<Option<HeaderHash>>) -> Result<()> {
        self.settle_with(observe, |_| Receipt::Unknown)
    }
    /// As `settle`, with what Core's journal knows about the publication at each path (audit A26):
    /// a version the journal shows as published is never replaced by an older copy found on disk.
    pub fn settle_with(&mut self, mut observe: impl FnMut(&str) -> Result<Option<HeaderHash>>,
        mut receipt: impl FnMut(&str) -> Receipt) -> Result<()> {
        let pending: Vec<(Locator, String)> = self.pending.iter().map(|(l, p)| (*l, p.clone())).collect();
        for (locator, path) in pending {
            let Some(record) = self.records.get(&locator).copied() else { continue };
            let seen = observe(&path)?;
            let known = receipt(&path);
            // Still running: decide later.
            if known == Receipt::InFlight { continue; }
            let next = match (record.state, seen) {
                (RecordState::Changing, Some(h)) if h == record.current => Some(record.current),
                // The old copy is found although the new one was published: a rollback on the
                // medium. Keep the published version so readers refuse the old one.
                (RecordState::Changing, Some(h)) if record.previous != ZERO && h == record.previous && known == Receipt::Published => Some(record.current),
                (RecordState::Changing, Some(h)) if record.previous != ZERO && h == record.previous => Some(record.previous),
                // A deletion the journal shows as done, yet the object is back: do not resurrect it.
                (RecordState::Deleting, Some(h)) if h == record.current && known == Receipt::Published => None,
                (RecordState::Changing, None) if record.previous == ZERO => None,
                (RecordState::Deleting, None) => None,
                (RecordState::Deleting, Some(h)) if h == record.current => Some(record.current),
                // Anything else is not explained by our own interrupted plan:
                // leave the record as it is so readers keep refusing the object.
                _ => continue,
            };
            match next {
                Some(current) => { self.records.insert(locator, Record { state: RecordState::Present, current, previous: ZERO }); }
                None => { self.records.remove(&locator); }
            }
            self.pending.remove(&locator);
        }
        Ok(())
    }
}

/// What Core's journal says about the publication behind a pending manifest record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt { Published, NotPublished, InFlight, Unknown }

/// Keyed identity of an entry object: storage folder path + object name.
pub fn locator(keys: &VaultKeys, folder_path: &str, object: &str) -> Result<Locator> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(keys.manifest_key()).map_err(|_| CryptoError::InvalidInput)?;
    mac.update(folder_path.as_bytes()); mac.update(b"/"); mac.update(object.as_bytes());
    let digest = mac.finalize().into_bytes();
    let mut out = [0u8; 16]; out.copy_from_slice(&digest[..16]);
    Ok(out)
}

/// Hash of an entry header (exactly the 374 header bytes).
pub fn header_hash(header: &[u8]) -> Result<HeaderHash> {
    if header.len() != ENTRY_HEADER_LEN { return Err(CryptoError::InvalidInput); }
    let mut out = [0u8; 16]; out.copy_from_slice(&blake3::hash(header).as_bytes()[..16]);
    Ok(out)
}

fn aad(keys: &VaultKeys) -> Vec<u8> { [keys.vault_id.as_slice(), AD_TAG].concat() }

/// Seal a manifest for this vault (fresh random nonce).
pub fn seal(keys: &VaultKeys, manifest: &Manifest, rng: &mut dyn RandomSource) -> Result<Vec<u8>> {
    if manifest.records.len() > MAX_RECORDS { return Err(CryptoError::LimitExceeded); }
    let mut body = Vec::with_capacity(BODY_FIXED + manifest.records.len() * RECORD_LEN);
    body.extend_from_slice(&manifest.sequence.to_le_bytes());
    body.extend_from_slice(&(manifest.records.len() as u32).to_le_bytes());
    for (locator, r) in &manifest.records {
        body.extend_from_slice(locator); body.push(r.state as u8); body.extend_from_slice(&r.current); body.extend_from_slice(&r.previous);
    }
    body.extend_from_slice(&(manifest.pending.len() as u32).to_le_bytes());
    for (locator, path) in &manifest.pending {
        body.extend_from_slice(locator); body.push(path.len() as u8); body.extend_from_slice(path.as_bytes());
    }
    let mut nonce = [0u8; 24]; rng.fill(&mut nonce)?;
    let cipher = XChaCha20Poly1305::new_from_slice(keys.manifest_key()).map_err(|_| CryptoError::InvalidInput)?;
    let sealed = cipher.encrypt(XNonce::from_slice(&nonce), Payload { msg: &body, aad: &aad(keys) }).map_err(|_| CryptoError::Authentication)?;
    let mut out = Vec::with_capacity(8 + 24 + sealed.len());
    out.extend_from_slice(MAGIC); out.extend_from_slice(&nonce); out.extend_from_slice(&sealed);
    Ok(out)
}

/// Records in flight at once (pending paths).
const MAX_PENDING: usize = 4096;
/// Largest valid sealed manifest.
pub const MAX_SEALED: u64 = (8 + 24 + BODY_FIXED + MAX_RECORDS * RECORD_LEN + 4 + MAX_PENDING * (16 + 1 + PATH_MAX) + TAG_LEN) as u64;

/// Open and fully validate a sealed manifest (sorted unique locators, known
/// states, exact length). Any deviation is an authentication/format failure.
pub fn open(keys: &VaultKeys, bytes: &[u8]) -> Result<Manifest> {
    if bytes.len() < 8 + 24 + BODY_FIXED + TAG_LEN || bytes.len() as u64 > MAX_SEALED || &bytes[..8] != MAGIC {
        return Err(CryptoError::InvalidInput);
    }
    let cipher = XChaCha20Poly1305::new_from_slice(keys.manifest_key()).map_err(|_| CryptoError::InvalidInput)?;
    let body = zeroize::Zeroizing::new(cipher.decrypt(XNonce::from_slice(&bytes[8..32]), Payload { msg: &bytes[32..], aad: &aad(keys) })
        .map_err(|_| CryptoError::Authentication)?);
    let mut sequence = [0u8; 8]; sequence.copy_from_slice(&body[..8]);
    let mut count = [0u8; 4]; count.copy_from_slice(&body[8..12]);
    let count = u32::from_le_bytes(count) as usize;
    let records_end = BODY_FIXED.checked_add(count.checked_mul(RECORD_LEN).ok_or(CryptoError::InvalidInput)?).ok_or(CryptoError::InvalidInput)?;
    if count > MAX_RECORDS || body.len() < records_end + 4 { return Err(CryptoError::InvalidInput); }
    let mut records = BTreeMap::new();
    let mut last: Option<Locator> = None;
    for chunk in body[BODY_FIXED..records_end].chunks_exact(RECORD_LEN) {
        let mut locator = [0u8; 16]; locator.copy_from_slice(&chunk[..16]);
        if last.is_some_and(|l| l >= locator) { return Err(CryptoError::InvalidInput); }
        last = Some(locator);
        let state = match chunk[16] { 0 => RecordState::Present, 1 => RecordState::Changing, 2 => RecordState::Deleting, _ => return Err(CryptoError::InvalidInput) };
        let mut current = [0u8; 16]; current.copy_from_slice(&chunk[17..33]);
        let mut previous = [0u8; 16]; previous.copy_from_slice(&chunk[33..49]);
        if state != RecordState::Changing && previous != ZERO { return Err(CryptoError::InvalidInput); }
        records.insert(locator, Record { state, current, previous });
    }
    let mut at = records_end;
    let mut pending_count = [0u8; 4]; pending_count.copy_from_slice(&body[at..at + 4]); at += 4;
    let pending_count = u32::from_le_bytes(pending_count) as usize;
    if pending_count > MAX_PENDING { return Err(CryptoError::InvalidInput); }
    let mut pending = BTreeMap::new();
    let mut last: Option<Locator> = None;
    for _ in 0..pending_count {
        if body.len() < at + 17 { return Err(CryptoError::InvalidInput); }
        let mut locator = [0u8; 16]; locator.copy_from_slice(&body[at..at + 16]);
        let len = body[at + 16] as usize; at += 17;
        if len == 0 || len > PATH_MAX || body.len() < at + len || last.is_some_and(|l| l >= locator) { return Err(CryptoError::InvalidInput); }
        last = Some(locator);
        let path = std::str::from_utf8(&body[at..at + len]).map_err(|_| CryptoError::InvalidInput)?.to_owned(); at += len;
        // Every record in flight has exactly one path, and only those do.
        if records.get(&locator).is_none_or(|r: &Record| r.state == RecordState::Present) { return Err(CryptoError::InvalidInput); }
        pending.insert(locator, path);
    }
    if at != body.len() || records.values().filter(|r| r.state != RecordState::Present).count() != pending.len() {
        return Err(CryptoError::InvalidInput);
    }
    Ok(Manifest { sequence: u64::from_le_bytes(sequence), records, pending })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::paranoic_v1::{create_vault, require_manifest, unlock, KdfParams, OsRandom};

    fn vault() -> (Vec<u8>, VaultKeys) {
        let kdf = KdfParams { memory_kib: 65_536, iterations: 1, lanes: 1 };
        create_vault(b"manifest test pw", kdf, false, &mut OsRandom, &mut || Ok(())).unwrap()
    }
    fn hash(n: u8) -> HeaderHash { [n; 16] }

    #[test] fn seal_open_round_trip_and_tamper() {
        let (_, keys) = vault();
        let mut m = Manifest { sequence: 7, ..Manifest::default() };
        for i in 0..5u8 { m.insert_present(locator(&keys, "d/aa/x", &format!("o{i}.f")).unwrap(), hash(i + 1)).unwrap(); }
        let sealed = seal(&keys, &m, &mut OsRandom).unwrap();
        assert_eq!(open(&keys, &sealed).unwrap(), m);
        for i in 0..sealed.len() {
            let mut bad = sealed.clone(); bad[i] ^= 0x01;
            assert!(open(&keys, &bad).is_err(), "byte {i} flip accepted");
        }
        assert!(open(&keys, &sealed[..sealed.len() - 1]).is_err());
        let (_, other) = vault();
        assert!(open(&other, &sealed).is_err(), "manifest of another vault accepted");
    }

    #[test] fn verdicts_and_settling() {
        let (_, keys) = vault();
        let a = locator(&keys, "d/aa/x", "a.f").unwrap();
        let b = locator(&keys, "d/aa/x", "b.f").unwrap();
        let mut m = Manifest::default();
        m.insert_present(a, hash(1)).unwrap();
        assert_eq!(m.check(&a, &hash(1)), Verdict::Accepted);
        assert_eq!(m.check(&a, &hash(9)), Verdict::Mismatch, "older/other version accepted");
        assert_eq!(m.check(&b, &hash(1)), Verdict::Unknown);
        // Change in flight: old and new both acceptable; settle to what is stored.
        m.begin_change(a, hash(2), "d/aa/x/a.f").unwrap();
        let sealed = seal(&keys, &m, &mut OsRandom).unwrap();
        assert_eq!(open(&keys, &sealed).unwrap(), m, "pending path lost in round trip");
        assert_eq!(m.check(&a, &hash(1)), Verdict::Accepted);
        assert_eq!(m.check(&a, &hash(2)), Verdict::Accepted);
        assert!(m.begin_change(a, hash(3), "d/aa/x/a.f").is_err(), "unsettled change overwritten");
        m.settle(|_| Ok(Some(hash(2)))).unwrap();
        assert_eq!(m.check(&a, &hash(1)), Verdict::Mismatch, "old version still accepted after settle");
        // Interrupted change: the storage still has the old version.
        m.begin_change(a, hash(3), "d/aa/x/a.f").unwrap();
        m.settle(|_| Ok(Some(hash(2)))).unwrap();
        assert_eq!(m.record(&a).unwrap().current, hash(2));
        // Audit A26: the journal says the new version was published, yet the old copy is on the
        // medium: the old copy is not adopted; and a running publication keeps the record pending.
        m.begin_change(a, hash(4), "d/aa/x/a.f").unwrap();
        m.settle_with(|_| Ok(Some(hash(2))), |_| Receipt::InFlight).unwrap();
        assert_eq!(m.check(&a, &hash(4)), Verdict::Accepted, "in-flight change settled too early");
        m.settle_with(|_| Ok(Some(hash(2))), |_| Receipt::Published).unwrap();
        assert_eq!(m.record(&a).unwrap().current, hash(4), "rolled-back copy adopted over a published version");
        assert_eq!(m.check(&a, &hash(2)), Verdict::Mismatch);
        m.begin_change(a, hash(2), "d/aa/x/a.f").unwrap();
        m.settle_with(|_| Ok(Some(hash(2))), |_| Receipt::Published).unwrap();
        // New object never published: the record disappears.
        m.begin_change(b, hash(5), "d/aa/x/b.f").unwrap();
        m.settle(|p| Ok(if p.ends_with("b.f") { None } else { Some(hash(2)) })).unwrap();
        assert!(m.record(&b).is_none());
        // Deletion settled: a later copy of the deleted object is Unknown.
        m.begin_delete(a, "d/aa/x/a.f").unwrap();
        assert_eq!(m.check(&a, &hash(2)), Verdict::Accepted, "object refused before the delete ran");
        m.settle(|_| Ok(None)).unwrap();
        assert_eq!(m.check(&a, &hash(2)), Verdict::Unknown, "deleted object resurrected");
        // Unexplained state is not settled away.
        m.begin_change(b, hash(6), "d/aa/x/b.f").unwrap();
        m.settle(|_| Ok(Some(hash(7)))).unwrap();
        assert_eq!(m.record(&b).unwrap().state, RecordState::Changing);
    }

    #[test] fn header_flag_requires_manifest() {
        let (header, keys) = vault();
        assert!(!keys.manifest_required);
        let upgraded = require_manifest(&header, &keys).unwrap();
        let reopened = unlock(&upgraded, b"manifest test pw", &mut || Ok(())).unwrap();
        assert!(reopened.manifest_required);
        // The flag is covered by the header MAC: clearing it is detected.
        let mut stripped = upgraded.clone(); stripped[26] &= !2;
        assert!(unlock(&stripped, b"manifest test pw", &mut || Ok(())).is_err(), "flag removed without detection");
        // Unknown flag bits are still refused.
        let mut unknown = upgraded; unknown[26] |= 4;
        assert!(unlock(&unknown, b"manifest test pw", &mut || Ok(())).is_err());
    }
}
