use std::{io::Read, ops::Range};
use zeroize::{Zeroizing, Zeroize};
use paranoic_crypto::StreamSink;
use crate::{CoreError, ErrorCode, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextEncoding { Utf8, Utf8Bom, Utf16Le, Utf16Be }

/// Plaintext is erased on drop. Markdown is text, never executable HTML here.
pub struct TextDocument {
    text: Zeroizing<String>, encoding: TextEncoding, limit: usize, revision: u64,
    /// Source version observed by `open_location`; "Save" replaces only this version.
    pub(crate) opened: Option<paranoic_core::Metadata>,
    original_blake3: [u8; 32],
}
impl TextDocument {
    pub fn open(mut input: impl Read, limit: usize, mut check: impl FnMut() -> Result<()>) -> Result<Self> {
        if limit == 0 || limit > 64 * 1024 * 1024 { return Err(invalid()); }
        let mut raw = Zeroizing::new(Vec::new());
        let mut buffer = Zeroizing::new(vec![0u8; 8192]);
        loop {
            check()?;
            let n = input.read(&mut buffer).map_err(CoreError::io)?;
            if n == 0 { break; }
            if raw.len().saturating_add(n) > limit { return Err(large()); }
            raw.extend_from_slice(&buffer[..n]);
        }
        let (encoding, offset) = if raw.starts_with(&[0xef,0xbb,0xbf]) { (TextEncoding::Utf8Bom, 3) }
            else if raw.starts_with(&[0xff,0xfe]) { (TextEncoding::Utf16Le, 2) }
            else if raw.starts_with(&[0xfe,0xff]) { (TextEncoding::Utf16Be, 2) }
            else { (TextEncoding::Utf8, 0) };
        let text = if matches!(encoding, TextEncoding::Utf8 | TextEncoding::Utf8Bom) {
            Zeroizing::new(std::str::from_utf8(&raw[offset..]).map_err(|_| invalid())?.to_owned())
        } else {
            if (raw.len() - offset) % 2 != 0 { return Err(invalid()); }
            let units = Zeroizing::new(raw[offset..].chunks_exact(2).map(|p| {
                if encoding == TextEncoding::Utf16Le { u16::from_le_bytes([p[0],p[1]]) }
                else { u16::from_be_bytes([p[0],p[1]]) }
            }).collect::<Vec<_>>());
            let mut decoded = Zeroizing::new(String::new());
            for (index, value) in char::decode_utf16(units.iter().copied()).enumerate() {
                if index % 4096 == 0 { check()?; }
                let value = value.map_err(|_| invalid())?;
                if decoded.len().saturating_add(value.len_utf8()) > limit { return Err(large()); }
                decoded.push(value);
            }
            decoded
        };
        if text.len() > limit || text.contains('\0') { return Err(invalid()); }
        check()?;
        let original_blake3 = *blake3::hash(&raw).as_bytes();
        Ok(Self { text, encoding, limit, revision: 0, opened: None, original_blake3 })
    }
    pub fn text(&self) -> &str { &self.text }
    pub fn encoding(&self) -> TextEncoding { self.encoding }
    pub fn revision(&self) -> u64 { self.revision }
    /// Metadata of the file when it was opened through Core (None for other sources).
    pub fn opened_metadata(&self) -> Option<&paranoic_core::Metadata> { self.opened.as_ref() }
    /// Digest of the exact original encoded bytes, including BOM (never edited).
    pub fn original_blake3(&self) -> &[u8; 32] { &self.original_blake3 }
    /// Byte offsets must be UTF-8 boundaries; stale editor revisions fail.
    pub fn replace(&mut self, revision: u64, range: Range<usize>, replacement: &str) -> Result<u64> {
        if revision != self.revision { return Err(CoreError::new(ErrorCode::SourceChanged, "document revision changed")); }
        if range.start > range.end || !self.text.is_char_boundary(range.start) || !self.text.is_char_boundary(range.end)
            || replacement.contains('\0') { return Err(invalid()); }
        if (self.text.len() - range.len()).saturating_add(replacement.len()) > self.limit { return Err(large()); }
        let next = self.revision.checked_add(1).ok_or_else(invalid)?;
        // Replace the owned allocation so removed plaintext is zeroized too.
        let mut edited = Zeroizing::new(String::with_capacity(self.text.len() - range.len() + replacement.len()));
        edited.push_str(&self.text[..range.start]); edited.push_str(replacement); edited.push_str(&self.text[range.end..]);
        self.text = edited; self.revision = next; Ok(next)
    }
    /// The host provides a tentative transactional sink, including conflict policy.
    /// Any error before commit explicitly aborts; commit errors remain uncertain.
    pub fn save(&self, mut sink: Box<dyn StreamSink>, mut check: impl FnMut() -> Result<()>) -> Result<()> {
        let write = (|| {
            check()?;
            let bom: &[u8] = match self.encoding { TextEncoding::Utf8 => &[], TextEncoding::Utf8Bom => &[239,187,191],
                TextEncoding::Utf16Le => &[255,254], TextEncoding::Utf16Be => &[254,255] };
            sink.write_all(bom).map_err(CoreError::io)?;
            if matches!(self.encoding, TextEncoding::Utf8 | TextEncoding::Utf8Bom) {
                for part in self.text.as_bytes().chunks(8192) { check()?; sink.write_all(part).map_err(CoreError::io)?; }
            } else {
                let mut buffer = Zeroizing::new(Vec::with_capacity(8192));
                for unit in self.text.encode_utf16() {
                    buffer.extend_from_slice(&if self.encoding == TextEncoding::Utf16Le { unit.to_le_bytes() } else { unit.to_be_bytes() });
                    if buffer.len() >= 8192 { check()?; sink.write_all(&buffer).map_err(CoreError::io)?; buffer.zeroize(); }
                }
                sink.write_all(&buffer).map_err(CoreError::io)?;
            }
            sink.flush().map_err(CoreError::io)?; check()
        })();
        if let Err(error) = write {
            sink.abort().map_err(|_| CoreError::new(ErrorCode::NeedsAttention, "document output cleanup failed"))?;
            return Err(error);
        }
        sink.commit().map_err(|_| CoreError::new(ErrorCode::NeedsAttention, "document publication uncertain"))
    }
}
fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "invalid text document or edit") }
fn large() -> CoreError { CoreError::new(ErrorCode::LimitExceeded, "text document exceeds limit") }
