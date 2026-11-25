//! Photo/audio/video support (roadmap 2.7) without plaintext in shared caches.
//!
//! * `sniff` classifies content by magic bytes, never by file extension alone.
//! * `decode_image` / `thumbnail_image` decode PNG/JPEG/GIF/WebP/BMP in pure
//!   Rust with dimension and allocation ceilings; EXIF orientation is applied
//!   and re-encoding strips all metadata (location, camera, comments).
//! * `ThumbnailCache` stores thumbnails as XChaCha20-Poly1305 envelopes in a
//!   host-owned PrivateDirectory under an EPHEMERAL in-memory key; object
//!   names are keyed hashes, so neither content nor source paths are visible.
//!   `lock()` (Quick Lock / app exit) drops both keys and removes the objects;
//!   leftovers after a crash are ciphertext under a lost key.
//! * `MediaStream` gives platform players (Android Media3 DataSource, Windows
//!   media engine/libVLC callbacks) random-access reads straight from a Core
//!   grant, so audio/video never needs a plaintext temporary file.
//!
//! Private Vault must use its own cache instance and directory.
use std::{collections::BTreeSet, io::{Cursor, Read}, sync::Mutex};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use paranoic_core::{Core, EntryKind, Location, Metadata};
use paranoic_crypto::{ObjectId, PrivateDirectory, SecretKey};
use zeroize::Zeroizing;
use crate::{CoreError, ErrorCode, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind { Image, Audio, Video }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaType { pub kind: MediaKind, pub mime: &'static str, /** Decodable by `decode_image`. */ pub decodable: bool }

/// Classify from the first bytes (64 are enough). None for unknown content.
pub fn sniff(header: &[u8]) -> Option<MediaType> {
    let h = header;
    let t = |kind, mime, decodable| Some(MediaType { kind, mime, decodable });
    if h.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) { return t(MediaKind::Image, "image/png", true); }
    if h.starts_with(&[0xff, 0xd8, 0xff]) { return t(MediaKind::Image, "image/jpeg", true); }
    if h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a") { return t(MediaKind::Image, "image/gif", true); }
    if h.starts_with(b"BM") && h.len() >= 14 { return t(MediaKind::Image, "image/bmp", true); }
    if h.len() >= 12 && &h[..4] == b"RIFF" {
        return match &h[8..12] { b"WEBP" => t(MediaKind::Image, "image/webp", true), b"WAVE" => t(MediaKind::Audio, "audio/wav", false),
            b"AVI " => t(MediaKind::Video, "video/x-msvideo", false), _ => None };
    }
    if h.len() >= 12 && &h[4..8] == b"ftyp" {
        return match &h[8..12] {
            b"heic" | b"heix" | b"mif1" | b"msf1" => t(MediaKind::Image, "image/heic", false),
            b"avif" | b"avis" => t(MediaKind::Image, "image/avif", false),
            b"M4A " | b"M4B " | b"M4P " => t(MediaKind::Audio, "audio/mp4", false),
            b"qt  " => t(MediaKind::Video, "video/quicktime", false),
            b"3gp4" | b"3gp5" | b"3gp6" | b"3g2a" => t(MediaKind::Video, "video/3gpp", false),
            _ => t(MediaKind::Video, "video/mp4", false),
        };
    }
    if h.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        let webm = h.windows(4).any(|w| w == b"webm");
        return t(MediaKind::Video, if webm { "video/webm" } else { "video/x-matroska" }, false);
    }
    if h.starts_with(b"fLaC") { return t(MediaKind::Audio, "audio/flac", false); }
    if h.starts_with(b"OggS") {
        let video = h.windows(6).any(|w| w == b"theora") || h.windows(8).any(|w| w == b"\x80theora");
        return if video { t(MediaKind::Video, "video/ogg", false) } else { t(MediaKind::Audio, "audio/ogg", false) };
    }
    if h.starts_with(b"ID3") { return t(MediaKind::Audio, "audio/mpeg", false); }
    if h.len() >= 2 && h[0] == 0xff && h[1] & 0xe0 == 0xe0 {
        // ADTS AAC has layer bits 00; MPEG audio layers are non-zero.
        return if h[1] & 0x06 == 0 { t(MediaKind::Audio, "audio/aac", false) } else { t(MediaKind::Audio, "audio/mpeg", false) };
    }
    if h.starts_with(&[0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11]) { return t(MediaKind::Video, "video/x-ms-asf", false); }
    None
}

#[derive(Clone, Copy, Debug)]
pub struct ImageLimits { pub input_bytes: u64, pub max_width: u32, pub max_height: u32, pub max_alloc: u64 }
impl Default for ImageLimits {
    fn default() -> Self { Self { input_bytes: 128 << 20, max_width: 16_384, max_height: 16_384, max_alloc: 512 << 20 } }
}

/// Decoded RGBA8 pixels, row-major. Pixels are zeroized on drop.
pub struct DecodedImage { pub width: u32, pub height: u32, pub rgba: Zeroizing<Vec<u8>> }

/// Decoded pixels allowed at once across all requests (audit 26): one 50 MP
/// photo, or several smaller ones, but never two 256 MiB decodes together.
const DECODE_BUDGET: u64 = 288 << 20;
static DECODE_USED: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
static DECODE_FREE: std::sync::Condvar = std::sync::Condvar::new();
/// Reservation of decoded-pixel memory; released on drop.
struct DecodeReservation(u64);
impl DecodeReservation {
    fn take(bytes: u64) -> Result<Self> {
        if bytes > DECODE_BUDGET { return Err(large()); }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut used = DECODE_USED.lock().map_err(|_| poisoned())?;
        while *used + bytes > DECODE_BUDGET {
            let now = std::time::Instant::now();
            if now >= deadline { return Err(CoreError::new(ErrorCode::Busy, "image decoder busy")); }
            used = DECODE_FREE.wait_timeout(used, deadline - now).map_err(|_| poisoned())?.0;
        }
        *used += bytes;
        Ok(Self(bytes))
    }
}
impl Drop for DecodeReservation {
    fn drop(&mut self) {
        if let Ok(mut used) = DECODE_USED.lock() { *used = used.saturating_sub(self.0); }
        DECODE_FREE.notify_all();
    }
}

fn decode(bytes: &[u8], limits: ImageLimits) -> Result<(DynamicImage, DecodeReservation)> {
    if bytes.len() as u64 > limits.input_bytes { return Err(large()); }
    let media = sniff(&bytes[..bytes.len().min(64)]).filter(|m| m.decodable).ok_or_else(unsupported)?;
    let format = match media.mime { "image/png" => ImageFormat::Png, "image/jpeg" => ImageFormat::Jpeg, "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP, "image/bmp" => ImageFormat::Bmp, _ => return Err(unsupported()) };
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut bounds = Limits::default();
    bounds.max_image_width = Some(limits.max_width); bounds.max_image_height = Some(limits.max_height); bounds.max_alloc = Some(limits.max_alloc);
    reader.limits(bounds);
    let mut decoder = reader.into_decoder().map_err(image_error)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || width > limits.max_width || height > limits.max_height
        || (width as u64) * (height as u64) * 4 > limits.max_alloc { return Err(large()); }
    // Reserved before the pixels exist; held while the caller uses the image.
    // Orientation may need a second buffer of the same size.
    let reservation = DecodeReservation::take((width as u64) * (height as u64) * 4)?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).map_err(image_error)?;
    if let Some(orientation) = orientation { image.apply_orientation(orientation); }
    Ok((image, reservation))
}
fn image_error(error: image::ImageError) -> CoreError {
    match error {
        image::ImageError::Limits(_) => large(),
        image::ImageError::Unsupported(_) => unsupported(),
        _ => CoreError::new(ErrorCode::InvalidInput, "image data is corrupt or truncated"),
    }
}

/// Full-resolution view for the image viewer (first frame of animations).
pub fn decode_image(bytes: &[u8], limits: ImageLimits) -> Result<DecodedImage> {
    let (image, _reserved) = decode(bytes, limits)?;
    let image = image.into_rgba8();
    let (width, height) = image.dimensions();
    Ok(DecodedImage { width, height, rgba: Zeroizing::new(image.into_raw()) })
}

/// PNG fitting `max_side` (16..=2048: thumbnails and the full-screen viewer), metadata stripped.
pub fn thumbnail_image(bytes: &[u8], max_side: u32, limits: ImageLimits) -> Result<Zeroizing<Vec<u8>>> {
    if !(16..=2048).contains(&max_side) { return Err(CoreError::new(ErrorCode::InvalidInput, "thumbnail size out of range")); }
    let (image, reserved) = decode(bytes, limits)?;
    let thumbnail = image.thumbnail(max_side, max_side);
    drop(image); drop(reserved);
    encode_png(thumbnail)
}

/// Thumbnail from a platform-decoded RGBA frame (e.g. a video frame from Media3).
pub fn thumbnail_rgba(width: u32, height: u32, rgba: &[u8], max_side: u32) -> Result<Zeroizing<Vec<u8>>> {
    if !(16..=1024).contains(&max_side) || width == 0 || height == 0 || width > 16_384 || height > 16_384
        || rgba.len() as u64 != width as u64 * height as u64 * 4 { return Err(CoreError::new(ErrorCode::InvalidInput, "invalid RGBA frame")); }
    let frame = image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(unsupported)?;
    let mut frame = DynamicImage::ImageRgba8(frame);
    let thumb = frame.thumbnail(max_side, max_side);
    if let DynamicImage::ImageRgba8(buffer) = &mut frame { zeroize::Zeroize::zeroize(&mut **buffer); }
    encode_png(thumb)
}
fn encode_png(image: DynamicImage) -> Result<Zeroizing<Vec<u8>>> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).map_err(image_error)?;
    Ok(Zeroizing::new(out.into_inner()))
}

/// Identity of a thumbnail: changes when the source object changes.
#[derive(Clone, Debug)]
pub struct ThumbnailKey { pub location: Location, pub bytes: u64, pub modified_unix_ms: Option<u64>, pub max_side: u32,
    /// BLAKE3 of the source bytes. Size and time alone do not identify a
    /// version (a replacement may keep both, a provider may report no time;
    /// audit 25), so `cached_thumbnail` always keys by content: the cache saves
    /// the decoding, never the check.
    pub content: Option<[u8; 32]> }
impl ThumbnailKey {
    pub fn from_metadata(location: Location, metadata: &Metadata, max_side: u32) -> Self {
        Self { location, bytes: metadata.bytes, modified_unix_ms: metadata.modified_unix_ms, max_side, content: None }
    }
}

/// Encrypted, session-scoped thumbnail cache. Never stores plaintext on disk.
pub struct ThumbnailCache { inner: Mutex<Option<CacheState>>, max_entry: usize, max_entries: usize, max_bytes: u64 }
/// `order`: insertion order for eviction (oldest first); `sizes`: envelope bytes.
struct CacheState { directory: PrivateDirectory, key: SecretKey, names: Zeroizing<[u8; 32]>, objects: BTreeSet<[u8; 16]>,
    order: std::collections::VecDeque<[u8; 16]>, sizes: std::collections::HashMap<[u8; 16], u64>, bytes: u64 }

impl ThumbnailCache {
    /// `directory` must be app-private, excluded from system backup, DEDICATED
    /// to this cache and not shared with Private Vault (own instance). Objects
    /// left by earlier sessions (crash; their key is gone) are removed first.
    /// Bounded to 2000 entries / 64 MiB; the oldest entries are evicted.
    pub fn ephemeral(directory: PrivateDirectory) -> Result<Self> {
        directory.clear_objects().map_err(crypto)?;
        let key = SecretKey::generate().map_err(crypto)?;
        // Independent random naming key (two 128-bit OS-random draws).
        let mut names = Zeroizing::new([0u8; 32]);
        names[..16].copy_from_slice(ObjectId::generate().map_err(crypto)?.as_bytes());
        names[16..].copy_from_slice(ObjectId::generate().map_err(crypto)?.as_bytes());
        Ok(Self { inner: Mutex::new(Some(CacheState { directory, key, names, objects: BTreeSet::new(),
            order: std::collections::VecDeque::new(), sizes: std::collections::HashMap::new(), bytes: 0 })),
            max_entry: 4 << 20, max_entries: 2000, max_bytes: 64 << 20 })
    }
    fn object(state: &CacheState, key: &ThumbnailKey) -> ObjectId {
        let mut hash = blake3::Hasher::new_keyed(&state.names);
        let path = key.location.path.as_str().as_bytes();
        hash.update(key.location.storage.0.as_bytes()); hash.update(&(path.len() as u64).to_le_bytes()); hash.update(path);
        hash.update(&key.bytes.to_le_bytes()); hash.update(&key.modified_unix_ms.map_or(u64::MAX, |v| v).to_le_bytes());
        hash.update(&key.max_side.to_le_bytes());
        hash.update(&[u8::from(key.content.is_some())]); hash.update(&key.content.unwrap_or([0; 32]));
        let mut id = [0u8; 16]; id.copy_from_slice(&hash.finalize().as_bytes()[..16]);
        ObjectId::from_bytes(id)
    }
    fn context(id: ObjectId) -> Vec<u8> { [&b"paranoic/thumbnail-cache/v1/"[..], id.as_bytes()].concat() }

    /// Cached PNG, or None (miss, locked, or failed authentication → removed).
    pub fn get(&self, key: &ThumbnailKey) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let mut guard = self.inner.lock().map_err(|_| poisoned())?;
        let Some(state) = guard.as_mut() else { return Ok(None) };
        let id = Self::object(state, key);
        if !state.objects.contains(id.as_bytes()) { return Ok(None); }
        let mut envelope = Vec::new();
        let read = state.directory.open_object(id).map_err(crypto)
            .and_then(|r| r.take(self.max_entry as u64 + 64).read_to_end(&mut envelope).map_err(CoreError::io));
        match read.ok().and_then(|_| state.key.open(&envelope, &Self::context(id)).ok()) {
            Some(png) => Ok(Some(png)),
            None => { state.forget(id.as_bytes()); let _ = state.directory.remove_object(id); Ok(None) }
        }
    }
    /// Store an encrypted PNG. Replaces nothing: an existing entry is kept.
    pub fn put(&self, key: &ThumbnailKey, png: &[u8]) -> Result<()> {
        if png.len() > self.max_entry { return Err(large()); }
        let mut guard = self.inner.lock().map_err(|_| poisoned())?;
        let state = guard.as_mut().ok_or_else(locked)?;
        let id = Self::object(state, key);
        if state.objects.contains(id.as_bytes()) { return Ok(()); }
        let envelope = state.key.seal(png, &Self::context(id)).map_err(crypto)?;
        let mut sink = state.directory.stage(id).map_err(crypto)?;
        if let Err(error) = std::io::Write::write_all(&mut sink, &envelope) { let _ = sink.abort(); return Err(CoreError::io(error)); }
        match sink.commit() {
            Ok(()) => {
                let size = envelope.len() as u64;
                state.objects.insert(*id.as_bytes()); state.order.push_back(*id.as_bytes());
                state.sizes.insert(*id.as_bytes(), size); state.bytes += size;
                // Evict oldest until within bounds (never the entry just written).
                while state.objects.len() > self.max_entries || state.bytes > self.max_bytes {
                    let Some(oldest) = state.order.pop_front() else { break };
                    if oldest == *id.as_bytes() { state.order.push_back(oldest); break; }
                    if state.forget(&oldest) { let _ = state.directory.remove_object(ObjectId::from_bytes(oldest)); }
                }
                Ok(())
            }
            Err(paranoic_crypto::CryptoError::AlreadyExists) => Ok(()),
            Err(error) => Err(crypto(error)),
        }
    }
    /// Quick Lock / exit: drop keys and remove every object of this session.
    /// Returns NeedsAttention if some objects could not be removed (they stay
    /// undecryptable). The cache is unusable afterwards.
    pub fn lock(&self) -> Result<()> {
        let state = self.inner.lock().map_err(|_| poisoned())?.take();
        let Some(state) = state else { return Ok(()) };
        let CacheState { directory, key, names, objects, .. } = state;
        drop(key); drop(names);
        let mut failed = false;
        for id in objects { if directory.remove_object(ObjectId::from_bytes(id)).is_err() { failed = true; } }
        if failed { Err(CoreError::new(ErrorCode::NeedsAttention, "thumbnail cache cleanup incomplete")) } else { Ok(()) }
    }
    pub fn is_locked(&self) -> bool { self.inner.lock().map(|g| g.is_none()).unwrap_or(true) }
}
impl CacheState {
    /// Drop bookkeeping for an object; true if it was tracked.
    fn forget(&mut self, id: &[u8; 16]) -> bool {
        let known = self.objects.remove(id);
        if let Some(size) = self.sizes.remove(id) { self.bytes = self.bytes.saturating_sub(size); }
        self.order.retain(|o| o != id);
        known
    }
}
impl Drop for ThumbnailCache { fn drop(&mut self) { let _ = self.lock(); } }

/// Generate (or fetch) a thumbnail for an image on a Core grant.
pub fn cached_thumbnail(core: &Core, cache: &ThumbnailCache, location: &Location, max_side: u32,
    limits: ImageLimits, mut check: impl FnMut() -> Result<()>) -> Result<Zeroizing<Vec<u8>>> {
    check()?;
    let metadata = core.metadata(location)?;
    if metadata.kind != EntryKind::File || metadata.bytes > limits.input_bytes { return Err(large()); }
    let mut key = ThumbnailKey::from_metadata(location.clone(), &metadata, max_side);
    let mut input = core.open_read(location)?;
    if input.metadata()? != metadata { return Err(changed()); }
    let mut bytes = Zeroizing::new(Vec::with_capacity(metadata.bytes as usize));
    let mut buffer = Zeroizing::new(vec![0u8; 65536]);
    loop {
        check()?;
        let n = input.read(&mut buffer).map_err(CoreError::io)?;
        if n == 0 { break; }
        if bytes.len() as u64 + n as u64 > metadata.bytes { return Err(changed()); }
        bytes.extend_from_slice(&buffer[..n]);
    }
    if bytes.len() as u64 != metadata.bytes || input.metadata()? != metadata { return Err(changed()); }
    key.content = Some(*blake3::hash(&bytes).as_bytes());
    if let Some(png) = cache.get(&key)? { return Ok(png); }
    check()?;
    let png = thumbnail_image(&bytes, max_side, limits)?;
    // An entry over the cache's size bound is simply not cached: it must not hide
    // an image that decoded fine. Every other failure (locked cache) still fails.
    match cache.put(&key, &png) {
        Err(error) if error.code == ErrorCode::LimitExceeded => {}
        other => other?,
    }
    Ok(png)
}

/// Random-access read stream over a Core grant for platform media players.
/// The object identity (length, mtime, kind) is pinned at open; any change
/// observed on reopen is SourceChanged. Not thread-safe: one per player.
pub struct MediaStream<'a> {
    core: &'a Core, location: Location, metadata: Metadata, media: Option<MediaType>,
    handle: Box<dyn paranoic_core::ReadHandle>, position: u64,
}
impl<'a> MediaStream<'a> {
    pub fn open(core: &'a Core, location: Location) -> Result<Self> {
        let mut handle = core.open_read(&location)?;
        let metadata = handle.metadata()?;
        if metadata.kind != EntryKind::File { return Err(unsupported()); }
        let mut header = [0u8; 64]; let mut filled = 0;
        while filled < header.len() {
            let n = handle.read(&mut header[filled..]).map_err(CoreError::io)?;
            if n == 0 { break; }
            filled += n;
        }
        let media = sniff(&header[..filled]);
        Ok(Self { core, location, metadata, media, handle, position: filled as u64 })
    }
    pub fn len(&self) -> u64 { self.metadata.bytes }
    pub fn is_empty(&self) -> bool { self.metadata.bytes == 0 }
    pub fn media_type(&self) -> Option<MediaType> { self.media }
    /// Read up to `buffer.len()` bytes at `offset`; 0 means end of stream.
    pub fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if offset >= self.metadata.bytes || buffer.is_empty() { return Ok(0); }
        if offset != self.position { self.reposition(offset)?; }
        let want = buffer.len().min((self.metadata.bytes - offset).min(usize::MAX as u64) as usize);
        let n = self.handle.read(&mut buffer[..want]).map_err(CoreError::io)?;
        if n == 0 { return Err(changed()); } // shorter than pinned length
        self.position += n as u64; Ok(n)
    }
    fn reposition(&mut self, offset: u64) -> Result<()> {
        match self.handle.seek_to(offset) {
            Ok(()) => { self.position = offset; return Ok(()); }
            Err(error) if error.code != ErrorCode::Unsupported => return Err(error),
            Err(_) => {}
        }
        if offset < self.position {
            let handle = self.core.open_read(&self.location)?;
            if handle.metadata()? != self.metadata { return Err(changed()); }
            self.handle = handle; self.position = 0;
        }
        let mut skip = offset - self.position; let mut scratch = Zeroizing::new(vec![0u8; 65536]);
        while skip > 0 {
            let n = self.handle.read(&mut scratch[..skip.min(65536) as usize]).map_err(CoreError::io)?;
            if n == 0 { return Err(changed()); }
            skip -= n as u64; self.position += n as u64;
        }
        Ok(())
    }
}

fn crypto(_: paranoic_crypto::CryptoError) -> CoreError { CoreError::new(ErrorCode::Io, "private cache operation failed") }
fn locked() -> CoreError { CoreError::new(ErrorCode::PermissionDenied, "thumbnail cache is locked") }
fn large() -> CoreError { CoreError::new(ErrorCode::LimitExceeded, "media exceeds configured limits") }
fn unsupported() -> CoreError { CoreError::new(ErrorCode::Unsupported, "media format is not supported here") }
fn changed() -> CoreError { CoreError::new(ErrorCode::SourceChanged, "media changed while reading") }
fn poisoned() -> CoreError { CoreError::new(ErrorCode::Internal, "thumbnail cache state unavailable") }
