//! Plan 2.6: large text files read-only, in rows, without loading them
//! whole. One pass builds an index (the byte offset of every 256th row);
//! `read` then decodes only the rows asked for. A row ends at a line break
//! or after `row_bytes` of text (cut at a character boundary), so a file
//! that is one huge line still pages. The file must not change between the
//! index and a read: the opened version is compared every time.
use std::io::Read;
use paranoic_core::{EntryKind, Metadata, ReadHandle};
use zeroize::Zeroizing;
use crate::{CoreError, ErrorCode, Result, TextEncoding};

#[derive(Clone, Copy, Debug)]
pub struct TextPagerLimits { pub file_bytes: u64, pub row_bytes: usize, pub rows_per_read: usize }
impl Default for TextPagerLimits {
    fn default() -> Self { Self { file_bytes: 1 << 30, row_bytes: 16 * 1024, rows_per_read: 2000 } }
}
const SAMPLE: u64 = 256;

pub struct TextPager { source: Metadata, encoding: TextEncoding, rows: u64, marks: Vec<u64>, limits: TextPagerLimits }

fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "not a text file in a supported encoding") }
fn changed() -> CoreError { CoreError::new(ErrorCode::SourceChanged, "text file changed since it was opened") }

/// Incremental decoder: bytes in, (char, byte offset just after it) out.
struct Decoder { encoding: TextEncoding, pending: Vec<u8>, offset: u64 }
impl Decoder {
    fn push(&mut self, bytes: &[u8], f: &mut dyn FnMut(char, u64) -> Result<bool>) -> Result<bool> {
        self.pending.extend_from_slice(bytes);
        let mut used = 0usize;
        let pending = std::mem::take(&mut self.pending);
        let result = (|| -> Result<bool> {
            match self.encoding {
                TextEncoding::Utf8 | TextEncoding::Utf8Bom => {
                    let valid = match std::str::from_utf8(&pending) {
                        Ok(s) => s,
                        Err(e) if e.error_len().is_none() => std::str::from_utf8(&pending[..e.valid_up_to()]).map_err(|_| invalid())?,
                        Err(_) => return Err(invalid()),
                    };
                    for c in valid.chars() {
                        used += c.len_utf8();
                        if !f(c, self.offset + used as u64)? { return Ok(false); }
                    }
                }
                TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
                    let unit = |i: usize| { let p = [pending[i], pending[i + 1]];
                        if self.encoding == TextEncoding::Utf16Le { u16::from_le_bytes(p) } else { u16::from_be_bytes(p) } };
                    let mut i = 0;
                    while i + 2 <= pending.len() {
                        let first = unit(i);
                        let (c, width) = if (0xD800..0xDC00).contains(&first) {
                            if i + 4 > pending.len() { break; }
                            let second = unit(i + 2);
                            (char::decode_utf16([first, second]).next().and_then(|r| r.ok()).ok_or_else(invalid)?, 4)
                        } else { (char::decode_utf16([first]).next().and_then(|r| r.ok()).ok_or_else(invalid)?, 2) };
                        i += width; used = i;
                        if !f(c, self.offset + used as u64)? { return Ok(false); }
                    }
                }
            }
            Ok(true)
        })();
        self.offset += used as u64;
        self.pending = pending[used..].to_vec();
        result
    }
}

/// Row boundaries over decoded characters; identical while indexing and reading.
struct Rows { row_bytes: usize, current: usize, open: bool }
impl Rows {
    /// Returns Some(start offset of the next row) when `c` (ending at `end`) closes a row
    /// or must begin a new one; `before` is the offset where `c` starts.
    fn step(&mut self, c: char, before: u64, end: u64) -> Option<(u64, bool)> {
        if c == '\n' { self.current = 0; self.open = false; return Some((end, true)); }
        if self.open && self.current + c.len_utf8() > self.row_bytes {
            self.current = c.len_utf8(); return Some((before, false));
        }
        self.open = true; self.current += c.len_utf8(); None
    }
}

impl TextPager {
    /// Metadata, encoding, where text starts, and the head bytes read past it.
    fn begin(handle: &mut dyn ReadHandle, limits: TextPagerLimits) -> Result<(Metadata, TextEncoding, u64, Vec<u8>)> {
        let source = handle.metadata()?;
        if source.kind != EntryKind::File { return Err(invalid()); }
        if source.bytes > limits.file_bytes { return Err(CoreError::new(ErrorCode::LimitExceeded, "text file too large")); }
        let mut head = [0u8; 3];
        let mut got = 0;
        while got < 3 { let n = handle.read(&mut head[got..]).map_err(CoreError::io)?; if n == 0 { break; } got += n; }
        let (encoding, start) = if got >= 3 && head == [0xEF, 0xBB, 0xBF] { (TextEncoding::Utf8Bom, 3) }
            else if got >= 2 && head[..2] == [0xFF, 0xFE] { (TextEncoding::Utf16Le, 2) }
            else if got >= 2 && head[..2] == [0xFE, 0xFF] { (TextEncoding::Utf16Be, 2) }
            else { (TextEncoding::Utf8, 0) };
        Ok((source, encoding, start, head[start as usize..got].to_vec()))
    }
    /// One pass over the opened file: encoding, row count, sampled offsets.
    /// No seeking, so a sequential provider stream can be indexed too.
    pub fn index(mut handle: Box<dyn ReadHandle>, limits: TextPagerLimits, mut check: impl FnMut() -> Result<()>) -> Result<Self> {
        if limits.row_bytes < 4 || limits.rows_per_read == 0 { return Err(invalid()); }
        let (source, encoding, start, head) = Self::begin(handle.as_mut(), limits)?;
        let mut decoder = Decoder { encoding, pending: Vec::new(), offset: start };
        let mut rows = Rows { row_bytes: limits.row_bytes, current: 0, open: false };
        let (mut count, mut marks, mut before) = (0u64, vec![start], start);
        let mut total = start;
        let declared = source.bytes;
        let mut feed = |bytes: &[u8], total: &mut u64| -> Result<()> {
            // Stop as soon as the stream outgrows the declared size (audit 8):
            // a growing file or an inexact provider never costs more than the limit.
            *total = total.checked_add(bytes.len() as u64).filter(|t| *t <= declared).ok_or_else(changed)?;
            decoder.push(bytes, &mut |c, end| {
                if c == '\0' { return Err(invalid()); }
                if let Some((next, _)) = rows.step(c, before, end) {
                    count += 1;
                    if count % SAMPLE == 0 { marks.push(next); }
                }
                before = end; Ok(true)
            }).map(|_| ())
        };
        feed(&head, &mut total)?;
        let mut buffer = Zeroizing::new(vec![0u8; 64 * 1024]);
        loop {
            check()?;
            let n = handle.read(&mut buffer).map_err(CoreError::io)?;
            if n == 0 { break; }
            feed(&buffer[..n], &mut total)?;
        }
        if !decoder.pending.is_empty() { return Err(invalid()); }
        if rows.open { count += 1; }
        if total != source.bytes || handle.metadata()? != source { return Err(changed()); }
        Ok(Self { source, encoding, rows: count, marks, limits })
    }
    pub fn rows(&self) -> u64 { self.rows }
    pub fn encoding(&self) -> TextEncoding { self.encoding }
    pub fn source(&self) -> &Metadata { &self.source }
    /// Rows `first .. first + count` (fewer at the end), without line breaks.
    /// `handle` must be a fresh open of the same file; a different version
    /// is refused. Needs random access.
    pub fn read(&self, mut handle: Box<dyn ReadHandle>, first: u64, count: usize,
        mut check: impl FnMut() -> Result<()>) -> Result<Vec<Zeroizing<String>>> {
        if count > self.limits.rows_per_read { return Err(CoreError::new(ErrorCode::LimitExceeded, "too many rows at once")); }
        if handle.metadata()? != self.source { return Err(changed()); }
        if first >= self.rows || count == 0 { return Ok(Vec::new()); }
        let mark = (first / SAMPLE) as usize;
        let offset = *self.marks.get(mark).ok_or_else(changed)?;
        handle.seek_to(offset)?;
        let wanted = first + count as u64;
        let mut row = mark as u64 * SAMPLE;
        let mut out: Vec<Zeroizing<String>> = Vec::with_capacity(count);
        let mut text = Zeroizing::new(String::new());
        let mut decoder = Decoder { encoding: self.encoding, pending: Vec::new(), offset };
        let mut rows = Rows { row_bytes: self.limits.row_bytes, current: 0, open: false };
        let mut before = offset;
        let mut buffer = Zeroizing::new(vec![0u8; 64 * 1024]);
        let mut done = false;
        while !done {
            check()?;
            let n = handle.read(&mut buffer).map_err(CoreError::io)?;
            if n == 0 { break; }
            done = !decoder.push(&buffer[..n], &mut |c, end| {
                let boundary = rows.step(c, before, end);
                before = end;
                if let Some((_, newline)) = boundary {
                    if row >= first {
                        let mut line = std::mem::take(&mut *text);
                        if newline && line.ends_with('\r') { line.pop(); }
                        out.push(Zeroizing::new(line));
                    } else { text.clear(); }
                    row += 1;
                    if row >= wanted { return Ok(false); }
                    if !newline { text.push(c); }
                } else if c != '\n' { text.push(c); }
                Ok(true)
            })?;
        }
        if !done && row < wanted && !text.is_empty() && row >= first { out.push(Zeroizing::new(std::mem::take(&mut *text))); }
        if handle.metadata()? != self.source { return Err(changed()); }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::io::Cursor;

    struct Mem { data: Vec<u8>, at: usize, meta: Metadata }
    impl Read for Mem {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = out.len().min(self.data.len() - self.at).min(7_777);
            out[..n].copy_from_slice(&self.data[self.at..self.at + n]); self.at += n; Ok(n)
        }
    }
    impl ReadHandle for Mem {
        fn metadata(&self) -> paranoic_core::Result<Metadata> { Ok(self.meta.clone()) }
        fn seek_to(&mut self, offset: u64) -> paranoic_core::Result<()> { self.at = offset as usize; Ok(()) }
    }
    fn mem(data: &[u8]) -> Box<dyn ReadHandle> {
        Box::new(Mem { data: data.to_vec(), at: 0, meta: Metadata { kind: EntryKind::File, bytes: data.len() as u64, modified_unix_ms: Some(1) } })
    }
    fn ok() -> Result<()> { Ok(()) }
    fn rows(p: &TextPager, data: &[u8], first: u64, count: usize) -> Vec<String> {
        p.read(mem(data), first, count, ok).unwrap().into_iter().map(|s| s.to_string()).collect()
    }

    #[test] fn pages_match_a_full_split_everywhere() {
        let mut text = String::new();
        for i in 0..3000 { text.push_str(&format!("строка {i} {}\r\n", "ж".repeat(i % 7))); }
        text.push_str("без перевода в конце");
        let data = text.as_bytes();
        let p = TextPager::index(mem(data), TextPagerLimits::default(), ok).unwrap();
        let expected: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_owned()).collect();
        assert_eq!(p.rows(), expected.len() as u64);
        for (first, count) in [(0, 5), (255, 3), (256, 1), (511, 600), (2998, 10), (3000, 5)] {
            let end = (first + count).min(expected.len());
            assert_eq!(rows(&p, data, first as u64, count), expected[first..end], "{first}+{count}");
        }
        assert!(rows(&p, data, 9999, 3).is_empty());
    }

    /// A provider stream without random access can still be indexed.
    #[test] fn index_needs_no_seek() {
        struct NoSeek(Mem);
        impl Read for NoSeek { fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> { self.0.read(out) } }
        impl ReadHandle for NoSeek {
            fn metadata(&self) -> paranoic_core::Result<Metadata> { self.0.metadata() }
        }
        let data = b"\xEF\xBB\xBFa\nb\nc".to_vec();
        let meta = Metadata { kind: EntryKind::File, bytes: data.len() as u64, modified_unix_ms: Some(1) };
        let p = TextPager::index(Box::new(NoSeek(Mem { data: data.clone(), at: 0, meta })), TextPagerLimits::default(), ok).unwrap();
        assert_eq!(p.rows(), 3);
        assert_eq!(rows(&p, &data, 2, 1), ["c"]);
    }

    #[test] fn one_huge_line_is_cut_at_character_boundaries() {
        let line = "я".repeat(50_000); // 100 000 bytes, no line break
        let limits = TextPagerLimits { row_bytes: 1000, ..TextPagerLimits::default() };
        let p = TextPager::index(mem(line.as_bytes()), limits, ok).unwrap();
        assert_eq!(p.rows(), 100);
        let all: Vec<String> = p.read(mem(line.as_bytes()), 0, 100, ok).unwrap().into_iter().map(|s| s.to_string()).collect();
        assert!(all.iter().all(|r| r.len() == 1000));
        assert_eq!(all.concat(), line);
    }

    #[test] fn utf16_bom_and_refusals() {
        let mut le = vec![0xFF, 0xFE];
        for u in "a\n😀b\nc".encode_utf16() { le.extend_from_slice(&u.to_le_bytes()); }
        let p = TextPager::index(mem(&le), TextPagerLimits::default(), ok).unwrap();
        assert_eq!(p.encoding(), TextEncoding::Utf16Le);
        assert_eq!(rows(&p, &le, 0, 9), ["a", "😀b", "c"]);
        let bom = b"\xEF\xBB\xBFone\ntwo\n";
        let p = TextPager::index(mem(bom), TextPagerLimits::default(), ok).unwrap();
        assert_eq!(rows(&p, bom, 1, 1), ["two"]);
        assert!(TextPager::index(mem(b"bin\0ary"), TextPagerLimits::default(), ok).is_err());
        assert!(TextPager::index(mem(&[0x61, 0xC3]), TextPagerLimits::default(), ok).is_err(), "cut UTF-8");
        let small = TextPagerLimits { file_bytes: 3, ..TextPagerLimits::default() };
        assert_eq!(TextPager::index(mem(b"four"), small, ok).err().unwrap().code, ErrorCode::LimitExceeded);
        // A different version of the file is refused.
        let p = TextPager::index(mem(b"a\nb\n"), TextPagerLimits::default(), ok).unwrap();
        assert_eq!(p.read(mem(b"a\nc\nx"), 0, 1, ok).err().unwrap().code, ErrorCode::SourceChanged);
        let _ = Cursor::new(0);
    }
}
