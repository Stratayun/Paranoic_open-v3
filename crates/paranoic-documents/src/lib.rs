//! Bounded document and archive services. Invoke off the UI thread.
mod text;
mod archive;
mod zip_guard;
mod other_archives;
mod sevenz_guard;
pub use other_archives::{SevenZipService, CodecLimits};
pub mod sevenz_isolated;
pub use sevenz_isolated::IsolatedSevenZip;
mod publication;
mod extraction;
mod office;
mod media;
pub use media::{sniff, decode_image, thumbnail_image, thumbnail_rgba, cached_thumbnail, MediaKind, MediaType, ImageLimits, DecodedImage, ThumbnailKey, ThumbnailCache, MediaStream};
pub use office::{open_office, open_office_location, OfficeLimits, OfficeKind, OfficeView, WordView, WorkbookView, SheetView, Block, Run};
pub use extraction::{ExtractedFile, PreparedExtraction};
pub use publication::{ArchiveOutput, PreparedArtifact};
pub use text::{TextDocument, TextEncoding};
mod text_pager;
pub use text_pager::{TextPager, TextPagerLimits};
pub use archive::{ZipService, ArchiveLimits, ArchiveEntry, ZipSource, PASSWORD_REQUIRED, WRONG_PASSWORD, is_password_required, is_wrong_password, any_encrypted};
pub use paranoic_core::{CoreError, ErrorCode, Result};

#[cfg(feature = "pdf")]
pub mod pdf;
pub mod pdf_client;
pub mod worker_process;

