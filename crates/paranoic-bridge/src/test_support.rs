use crate::*;

pub(super) struct NoSealer;
impl RecordSealer for NoSealer {
    fn established(&self) -> bool { false }
    fn available(&self) -> bool { false }
    fn seal(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
    fn open(&self, _: Vec<u8>) -> HostBytes { HostBytes { error: 2, data: vec![] } }
}
