//! Platform-independent, durable file-operation core.
//!
//! Hosts explicitly register storage grants and choose a private application-state
//! directory. User paths are always relative to a grant. No global runtime,
//! networking, UI, platform permission acquisition, or implicit retries live here.

mod bootstrap;
mod config;
mod engine;
mod error;
mod journal;
mod model;
mod path;
mod storage;
mod tree;

pub use config::CoreConfig;
pub use engine::Core;
pub use journal::JournalSealer;
pub use error::{CoreError, ErrorCode, Result};
pub use model::*;
pub use path::ResourcePath;
pub use storage::*;
pub use tree::{TreeInventory, TreeLimits};
