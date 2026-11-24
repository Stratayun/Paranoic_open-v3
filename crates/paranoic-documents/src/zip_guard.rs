//! Bound central-directory work before zip-rs allocates its index.
use std::io::{Read, Seek, SeekFrom};
use crate::{ArchiveLimits, CoreError, ErrorCode, Result};
fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidInput, "invalid or split ZIP directory") }
fn u16le(b: &[u8]) -> u16 { u16::from_le_bytes([b[0],b[1]]) }
fn u32le(b: &[u8]) -> u32 { u32::from_le_bytes([b[0],b[1],b[2],b[3]]) }
fn u64le(b: &[u8]) -> u64 { u64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]]) }
fn at(input: &mut (impl Read + Seek), offset: u64, bytes: &mut [u8]) -> Result<()> {
    input.seek(SeekFrom::Start(offset)).map_err(CoreError::io)?;
    input.read_exact(bytes).map_err(CoreError::io)
}
pub(crate) fn inspect(input: &mut (impl Read + Seek), limits: ArchiveLimits,
    check: &mut impl FnMut() -> Result<()>) -> Result<()> {
    check()?;
    let length = input.seek(SeekFrom::End(0)).map_err(CoreError::io)?;
    if length > limits.archive_bytes { return Err(large()); }
    if length < 22 { return Err(invalid()); }
    let size = length.min(65535 + 22) as usize;
    let mut tail = vec![0u8; size]; at(input, length - size as u64, &mut tail)?;
    let end = (0..=size-22).rev().find(|&i| tail[i..i+4] == [0x50,0x4b,5,6]
        && i + 22 + u16le(&tail[i+20..i+22]) as usize == size).ok_or_else(invalid)?;
    let e = &tail[end..end+22];
    let eocd = length - size as u64 + end as u64;
    if u16le(&e[4..]) != 0 || u16le(&e[6..]) != 0 || u16le(&e[8..]) != u16le(&e[10..]) { return Err(invalid()); }
    let mut count = u16le(&e[10..]) as u64;
    let mut bytes = u32le(&e[12..]) as u64;
    let mut offset = u32le(&e[16..]) as u64;
    let mut boundary = eocd;
    let mut preceding = [0u8;4];
    if eocd >= 20 { at(input, eocd-20, &mut preceding)?; }
    if preceding == [0x50,0x4b,6,7] || count == 65535 || bytes == u32::MAX as u64 || offset == u32::MAX as u64 {
        if eocd < 20 { return Err(invalid()); }
        let mut locator = [0u8;20]; at(input, eocd-20, &mut locator)?;
        if locator[..4] != [0x50,0x4b,6,7] || u32le(&locator[4..]) != 0 || u32le(&locator[16..]) != 1 { return Err(invalid()); }
        boundary = u64le(&locator[8..]);
        if boundary.checked_add(56).is_none_or(|n| n > eocd-20) { return Err(invalid()); }
        let mut record = [0u8;56]; at(input, boundary, &mut record)?;
        if record[..4] != [0x50,0x4b,6,6] || u64le(&record[4..]) < 44
            || boundary.checked_add(12).and_then(|n| n.checked_add(u64le(&record[4..]))) != Some(eocd-20)
            || u32le(&record[16..]) != 0 || u32le(&record[20..]) != 0
            || u64le(&record[24..]) != u64le(&record[32..]) { return Err(invalid()); }
        let count64 = u64le(&record[32..]); let bytes64 = u64le(&record[40..]); let offset64 = u64le(&record[48..]);
        if (count != 65535 && count != count64) || (bytes != u32::MAX as u64 && bytes != bytes64)
            || (offset != u32::MAX as u64 && offset != offset64) { return Err(invalid()); }
        count = count64; bytes = bytes64; offset = offset64;
    }
    if count > limits.entries as u64 || bytes > limits.directory_bytes { return Err(large()); }
    let stop = offset.checked_add(bytes).ok_or_else(invalid)?;
    if stop > boundary || stop > length { return Err(invalid()); }
    let mut cursor = offset;
    let mut names = std::collections::BTreeMap::new();
    for _ in 0..count {
        check()?;
        if cursor.checked_add(46).is_none_or(|n| n > stop) { return Err(invalid()); }
        let mut header = [0u8;46]; at(input, cursor, &mut header)?;
        if header[..4] != [0x50,0x4b,1,2] || u16le(&header[34..]) != 0 { return Err(invalid()); }
        let name = u16le(&header[28..]) as u64;
        if name == 0 || name > 4096 { return Err(large()); }
        let next = cursor.checked_add(46 + name + u16le(&header[30..]) as u64 + u16le(&header[32..]) as u64).ok_or_else(invalid)?;
        if next > stop { return Err(invalid()); }
        let mut name_bytes = vec![0u8; name as usize];
        at(input, cursor + 46, &mut name_bytes)?;
        let text = std::str::from_utf8(&name_bytes).map_err(|_| invalid())?;
        let directory = text.ends_with('/');
        let text = if directory { &text[..text.len()-1] } else { text };
        let path = paranoic_core::ResourcePath::parse(text)?;
        crate::archive::validate_name(&mut names, &path, directory, limits.depth)?;
        cursor = next;
    }
    if cursor != stop { return Err(invalid()); }
    check()?; input.seek(SeekFrom::Start(0)).map_err(CoreError::io)?; Ok(())
}
fn large() -> CoreError { CoreError::new(ErrorCode::LimitExceeded, "ZIP index exceeds configured limits") }
