//! Plan 2.3 (bridge part): what is in a chosen folder, before any password.
//! The phone showed "wrong password" when the chosen folder held vaults one
//! level down; the UI now asks first and says "no vault here" or offers the
//! vaults found. Only marker files are checked: nothing is decrypted.
use crate::*;
use paranoic_core::ErrorCode;

pub const VAULT_KIND_PARANOIC: u8 = 1;
pub const VAULT_KIND_CRYPTOMATOR: u8 = 2;
/// Subfolders inspected at most (each costs a few provider calls).
const CHILD_LIMIT: usize = 64;

/// `location` is the vault root to pass to `vault_open` / `cryptomator_open`.
#[derive(Clone, uniffi::Record)]
pub struct NativeFoundVault { pub location: NativeLocation, pub name: String, pub kind: u8 }
/// `vaults`: the folder itself (one entry, empty name) or vaults in its
/// subfolders. `unchecked`: subfolders over the limit that were not checked.
#[derive(Clone, uniffi::Record)]
pub struct NativeVaultProbe { pub vaults: Vec<NativeFoundVault>, pub unchecked: u64 }

/// Marker files: 1.8 `paranoic.vault`; Cryptomator `vault.cryptomator`
/// (format 8) or only `masterkey.cryptomator` (format 7).
fn kinds(core: &Core, folder: &Location) -> Result<Vec<u8>> {
    let file = |name: &str| -> Result<bool> {
        let at = Location { storage: folder.storage, path: folder.path.join(name)? };
        match core.metadata(&at) {
            Ok(meta) => Ok(meta.kind == EntryKind::File),
            Err(error) if error.code == ErrorCode::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    };
    let mut found = Vec::new();
    if file(paranoic_formats::paranoic_v1::VAULT_FILE)? { found.push(VAULT_KIND_PARANOIC); }
    if file("vault.cryptomator")? || file("masterkey.cryptomator")? { found.push(VAULT_KIND_CRYPTOMATOR); }
    Ok(found)
}

#[uniffi::export]
impl NativeRuntime {
    /// Vaults in `folder` itself, otherwise in its direct subfolders.
    pub fn vault_probe(&self, folder: NativeLocation) -> Result<NativeVaultProbe> {
        let folder = folder.core()?;
        if self.core.metadata(&folder)?.kind != EntryKind::Directory { return Err(BridgeError::Invalid); }
        let own = kinds(&self.core, &folder)?;
        if !own.is_empty() {
            return Ok(NativeVaultProbe { unchecked: 0, vaults: own.into_iter()
                .map(|kind| NativeFoundVault { location: folder.clone().into(), name: String::new(), kind }).collect() });
        }
        let listing = self.core.list_for_display(&folder)?;
        let mut folders: Vec<_> = listing.entries.into_iter().filter(|e| e.metadata.kind == EntryKind::Directory).collect();
        folders.sort_by(|a, b| a.location.path.as_str().cmp(b.location.path.as_str()));
        let unchecked = folders.len().saturating_sub(CHILD_LIMIT) as u64;
        let mut vaults = Vec::new();
        for entry in folders.into_iter().take(CHILD_LIMIT) {
            let name = entry.location.path.parent_and_name()?.1.to_owned();
            // A subfolder the provider cannot describe is skipped, not fatal.
            let Ok(found) = kinds(&self.core, &entry.location) else { continue };
            for kind in found {
                vaults.push(NativeFoundVault { location: entry.location.clone().into(), name: name.clone(), kind });
            }
        }
        Ok(NativeVaultProbe { vaults, unchecked })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::fs;
    struct NoSealer;
    impl RecordSealer for NoSealer {
        fn available(&self) -> bool { false }
        fn seal(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
        fn open(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
    }
    struct NoDocs;
    fn info() -> HostInfo { HostInfo { error: 2, kind: 0, bytes: 0, modified_ms: None } }
    fn created() -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
    impl AndroidDocuments for NoDocs {
        fn metadata(&self, _: String, _: String) -> HostInfo { info() }
        fn list(&self, _: String, _: String, _: u32) -> HostListing { HostListing { error: 2, entries: vec![] } }
        fn open(&self, _: String, _: String) -> HostOpened { HostOpened { error: 2, id: 0, info: info() } }
        fn read(&self, _: u64, _: u64, _: u32) -> HostBytes { HostBytes { error: 2, data: vec![] } }
        fn file_metadata(&self, _: u64) -> HostInfo { info() }
        fn close(&self, _: u64) {}
        fn writable(&self, _: String) -> bool { false }
        fn create(&self, _: String, _: String, _: String, _: bool) -> HostCreated { created() }
        fn open_write(&self, _: String, _: String) -> HostOpened { HostOpened { error: 2, id: 0, info: info() } }
        fn write(&self, _: u64, _: Vec<u8>) -> u32 { 2 }
        fn finish_write(&self, _: u64) -> u32 { 2 }
        fn rename(&self, _: String, _: String, _: String) -> HostCreated { created() }
        fn delete(&self, _: String, _: String) -> u32 { 2 }
        fn resolve(&self, _: String, _: String) -> HostCreated { created() }
        fn move_document(&self, _: String, _: String, _: String, _: String) -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
    }

    /// The phone case: the chosen folder holds vaults one level down.
    #[test] fn finds_vaults_in_the_folder_or_its_subfolders_before_any_password() {
        let root = std::env::temp_dir().join(format!("paranoic-probe-{}", Uuid::new_v4()));
        let ws = root.join("workspace");
        for d in ["state", "workspace/outer/Mine", "workspace/outer/Theirs/d", "workspace/outer/Old",
            "workspace/outer/Plain", "workspace/empty"] { fs::create_dir_all(root.join(d)).unwrap(); }
        fs::write(ws.join("outer/Mine/paranoic.vault"), b"header").unwrap();
        fs::write(ws.join("outer/Theirs/vault.cryptomator"), b"jwt").unwrap();
        fs::write(ws.join("outer/Theirs/masterkey.cryptomator"), b"{}").unwrap();
        fs::write(ws.join("outer/Old/masterkey.cryptomator"), b"{}").unwrap();
        fs::write(ws.join("outer/Plain/paranoic.vault"), b"").unwrap();
        fs::remove_file(ws.join("outer/Plain/paranoic.vault")).unwrap();
        fs::create_dir(ws.join("outer/Plain/vault.cryptomator")).unwrap(); // a folder is not a marker
        fs::write(ws.join("outer/file.txt"), b"x").unwrap();
        let rt = NativeRuntime::new(root.join("state").to_str().unwrap().into(), ws.to_str().unwrap().into(),
            String::new(), String::new(), vec![], Arc::new(NoDocs), Arc::new(NoSealer)).unwrap();
        let app = rt.spaces().unwrap()[0].root.clone();
        let at = |p: &str| NativeLocation { storage_id: app.storage_id.clone(), path: p.into() };

        let outer = rt.vault_probe(at("outer")).unwrap();
        let found: Vec<(String, String, u8)> = outer.vaults.iter().map(|v| (v.name.clone(), v.location.path.clone(), v.kind)).collect();
        assert_eq!(found, [("Mine".into(), "outer/Mine".into(), VAULT_KIND_PARANOIC),
            ("Old".into(), "outer/Old".into(), VAULT_KIND_CRYPTOMATOR),
            ("Theirs".into(), "outer/Theirs".into(), VAULT_KIND_CRYPTOMATOR)]);
        assert_eq!(outer.unchecked, 0);
        // The vault folder itself: one entry pointing at it.
        let own = rt.vault_probe(at("outer/Theirs")).unwrap();
        assert_eq!(own.vaults.len(), 1);
        assert_eq!((own.vaults[0].location.path.as_str(), own.vaults[0].kind), ("outer/Theirs", VAULT_KIND_CRYPTOMATOR));
        // Nothing here: an empty answer, not an error and not "wrong password".
        assert!(rt.vault_probe(at("empty")).unwrap().vaults.is_empty());
        assert!(matches!(rt.vault_probe(at("outer/file.txt")), Err(BridgeError::Invalid)));
        rt.shutdown().unwrap(); drop(rt);
        let _ = fs::remove_dir_all(&root);
    }
}
