//! PDFium engine used ONLY inside the out-of-process worker
//! (`paranoic-pdf-worker`). The application process never loads PDFium; it
//! talks to the worker through `pdf_client`. No JS, forms, links or network.
use pdfium_render::prelude::*;
use std::io::Cursor;

pub const MAX_INPUT: u64 = 64 << 20;
const MAX_PIXELS: u64 = 16_777_216;

pub enum Failure { Invalid, Limit }

pub fn library(path: &std::path::Path) -> Option<Pdfium> {
    if !path.is_absolute() { return None; }
    Pdfium::bind_to_library(path).ok().map(Pdfium::new)
}
pub fn page_count(document: &PdfDocument<'_>) -> u32 { document.pages().len() as u32 }

pub fn render(document: &PdfDocument<'_>, index: u32, width: u32) -> Result<Vec<u8>, Failure> {
    if !(128..=4096).contains(&width) { return Err(Failure::Limit); }
    let count = page_count(document);
    if index >= count { return Err(Failure::Limit); }
    let page = document.pages().get(index as i32).map_err(|_| Failure::Invalid)?;
    let bitmap = page.render_with_config(&PdfRenderConfig::new().set_target_width(width as i32).set_maximum_height(4096))
        .map_err(|_| Failure::Invalid)?;
    let (w, h) = (bitmap.width() as u32, bitmap.height() as u32);
    if w == 0 || h == 0 || w as u64 * h as u64 > MAX_PIXELS { return Err(Failure::Limit); }
    let rgba = zeroize::Zeroizing::new(bitmap.as_rgba_bytes());
    let mut output = Cursor::new(Vec::new());
    image::write_buffer_with_format(&mut output, &rgba, w, h, image::ColorType::Rgba8, image::ImageFormat::Png)
        .map_err(|_| Failure::Invalid)?;
    Ok(output.into_inner())
}

/// Pages in [start, start+16) whose text contains `query` (case-insensitive).
pub fn search(document: &PdfDocument<'_>, query: &str, start: u32) -> Result<(Vec<u32>, Option<u32>), Failure> {
    if query.is_empty() || query.len() > 1024 { return Err(Failure::Invalid); }
    let count = page_count(document);
    if start > count { return Err(Failure::Limit); }
    let end = start.saturating_add(16).min(count);
    let query = query.to_lowercase();
    let mut pages = Vec::new();
    for index in start..end {
        let page = document.pages().get(index as i32).map_err(|_| Failure::Invalid)?;
        let text = page.text().map_err(|_| Failure::Invalid)?;
        if text.len() > 1_000_000 { return Err(Failure::Limit); }
        let text = zeroize::Zeroizing::new(text.all());
        if text.to_lowercase().contains(&query) { pages.push(index); }
    }
    Ok((pages, (end < count).then_some(end)))
}
