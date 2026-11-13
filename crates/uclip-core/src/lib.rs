//! Application use cases without a WebView or operating-system integration.
mod contracts;
mod history;
mod service;
mod storage;

pub use contracts::*;
pub use history::{ClipPayload, MAX_IMAGE_PIXELS, MAX_TEXT_BYTES};
pub use service::CoreService;
pub use storage::{Storage, SCHEMA_VERSION};
