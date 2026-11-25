use crate::*;

pub(super) struct NoDocs;
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
pub(super) fn info() -> HostInfo { HostInfo { error: 2, kind: 0, bytes: 0, modified_ms: None } }
pub(super) fn created() -> HostCreated { HostCreated { error: 2, name: String::new(), document: String::new() } }
pub(super) struct NoSealer;
impl RecordSealer for NoSealer {
    fn established(&self) -> bool { false }
    fn available(&self) -> bool { false }
    fn seal(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
    fn open(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
}
