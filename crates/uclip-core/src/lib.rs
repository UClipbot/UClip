//! Application use cases without a WebView or operating-system integration.
mod collections;
mod contracts;
mod desktop;
mod history;
mod keyboard;
mod service;
mod storage;

pub use contracts::*;
pub use desktop::WindowPlacement;
pub use history::{ClipPayload, MAX_IMAGE_PIXELS, MAX_TEXT_BYTES};
pub use keyboard::*;
pub use service::CoreService;
pub use storage::{Storage, SCHEMA_VERSION};
