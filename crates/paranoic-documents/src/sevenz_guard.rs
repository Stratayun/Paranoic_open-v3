//! Bounded pre-check of a 7z "next header" before sevenz-rust2 decodes it.
//! A plain header (kHeader) is already bounded by its stored size. An encoded
//! header (kEncodedHeader) is a compressed stream the library would inflate;
//! here its StreamsInfo is parsed WITHOUT decompression and rejected when the
//! declared unpacked size exceeds the index limit or an LZMA/LZMA2 dictionary
//! exceeds 64 MiB. Unknown or unusual structure is refused (strict profile).
use crate::{CoreError, ErrorCode, Result};

const MAX_DICTIONARY: u64 = 64 << 20;
/// Streams inside one header folder (a header is one or a few streams).
const MAX_SUBSTREAMS: u64 = 16;

struct Cursor<'a> { bytes: &'a [u8], at: usize }
impl Cursor<'_> {
    fn byte(&mut self) -> Result<u8> { let b = *self.bytes.get(self.at).ok_or_else(invalid)?; self.at += 1; Ok(b) }
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.bytes.len()).ok_or_else(invalid)?;
        let slice = &self.bytes[self.at..end]; self.at = end; Ok(slice)
    }
    /// 7z variable-length number (ReadNumber).
    fn number(&mut self) -> Result<u64> {
        let first = self.byte()?;
        let mut mask = 0x80u8; let mut value = 0u64;
        for i in 0..8 {
            if first & mask == 0 { return Ok(value | (u64::from(first & mask.wrapping_sub(1)) << (8 * i))); }
            value |= u64::from(self.byte()?) << (8 * i);
            mask >>= 1;
        }
        Ok(value)
    }
    fn small(&mut self, max: u64) -> Result<usize> {
        let n = self.number()?; if n > max { return Err(invalid()); } Ok(n as usize)
    }
}

/// `header` is the raw next header (already bounded by `directory_bytes`).
pub(crate) fn check(header: &[u8], directory_bytes: u64) -> Result<()> {
    if header.is_empty() { return Ok(()); } // empty archive: nothing to decode
    let mut c = Cursor { bytes: header, at: 0 };
    match c.byte()? {
        0x01 => return Ok(()), // kHeader: plain, size already bounded
        0x17 => {}             // kEncodedHeader
        _ => return Err(invalid()),
    }
    let mut unpacked_total = 0u64;
    // Per folder: whether UnpackInfo stored its CRC (needed by SubStreamsInfo).
    let mut folder_crc: Vec<bool> = Vec::new();
    loop {
        match c.byte()? {
            0x00 => break,
            0x06 => { // PackInfo
                c.number()?; let pack_streams = c.small(16)?;
                loop {
                    match c.byte()? {
                        0x00 => break,
                        0x09 => { for _ in 0..pack_streams { c.number()?; } }
                        0x0A => { digests(&mut c, pack_streams)?; }
                        _ => return Err(invalid()),
                    }
                }
            }
            0x07 => { // UnpackInfo
                if c.byte()? != 0x0B { return Err(invalid()); }
                let folders = c.small(4)?;
                if c.byte()? != 0 { return Err(invalid()); } // external
                let mut outs = Vec::with_capacity(folders);
                for _ in 0..folders {
                    let coders = c.small(4)?;
                    let (mut total_in, mut total_out) = (0usize, 0usize);
                    for _ in 0..coders {
                        let flags = c.byte()?;
                        if flags & 0xC0 != 0 { return Err(invalid()); }
                        let id = c.take(usize::from(flags & 0x0F))?.to_vec();
                        let (inputs, outputs) = if flags & 0x10 != 0 { (c.small(4)?, c.small(4)?) } else { (1, 1) };
                        total_in += inputs; total_out += outputs;
                        if flags & 0x20 != 0 {
                            let size = c.small(64)?;
                            let props = c.take(size)?;
                            if dictionary(&id, props)? > MAX_DICTIONARY { return Err(large()); }
                        }
                    }
                    if total_out == 0 { return Err(invalid()); }
                    for _ in 0..total_out - 1 { c.number()?; c.number()?; } // bind pairs
                    let packed = total_in.checked_sub(total_out - 1).ok_or_else(invalid)?;
                    if packed > 1 { for _ in 0..packed { c.number()?; } }
                    outs.push(total_out);
                }
                if c.byte()? != 0x0C { return Err(invalid()); }
                for count in outs {
                    for _ in 0..count {
                        let size = c.number()?;
                        if size > directory_bytes { return Err(large()); }
                        unpacked_total = unpacked_total.checked_add(size).ok_or_else(large)?;
                    }
                }
                folder_crc = vec![false; folders];
                loop {
                    match c.byte()? {
                        0x00 => break,
                        0x0A => folder_crc = digests(&mut c, folders)?,
                        _ => return Err(invalid()),
                    }
                }
            }
            0x08 => { // SubStreamsInfo (after UnpackInfo): bounded counts and sizes
                if folder_crc.is_empty() { return Err(invalid()); }
                let mut streams = vec![1usize; folder_crc.len()];
                loop {
                    match c.byte()? {
                        0x00 => break,
                        0x0D => { for n in streams.iter_mut() { *n = c.small(MAX_SUBSTREAMS)?; } }
                        0x09 => {
                            for n in &streams {
                                for _ in 1..*n { if c.number()? > directory_bytes { return Err(large()); } }
                            }
                        }
                        0x0A => {
                            // Digests only for streams whose CRC the folder does not already give.
                            let count = streams.iter().zip(&folder_crc)
                                .map(|(n, crc)| if *n == 1 && *crc { 0 } else { *n }).sum();
                            digests(&mut c, count)?;
                        }
                        _ => return Err(invalid()),
                    }
                }
            }
            _ => return Err(invalid()),
        }
    }
    if unpacked_total > directory_bytes.saturating_mul(4) { return Err(large()); }
    Ok(())
}

/// Skips a digest list; returns which items have a CRC.
fn digests(c: &mut Cursor<'_>, count: usize) -> Result<Vec<bool>> {
    let defined: Vec<bool> = if c.byte()? != 0 { vec![true; count] } else {
        let bits = c.take(count.div_ceil(8))?;
        (0..count).map(|i| bits[i / 8] & (0x80 >> (i % 8)) != 0).collect()
    };
    c.take(defined.iter().filter(|d| **d).count() * 4)?;
    Ok(defined)
}
fn dictionary(id: &[u8], props: &[u8]) -> Result<u64> {
    Ok(match id {
        [0x21] if props.len() == 1 => {
            let p = props[0];
            if p > 40 { return Err(invalid()); }
            if p == 40 { u64::from(u32::MAX) } else { (2u64 | u64::from(p & 1)) << (u32::from(p) / 2 + 11) }
        }
        [3, 1, 1] if props.len() == 5 => u64::from(u32::from_le_bytes([props[1], props[2], props[3], props[4]])),
        _ => 0,
    })
}
fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "7z header layout rejected") }
fn large() -> CoreError { CoreError::new(ErrorCode::LimitExceeded, "7z header exceeds configured limits") }
