//! Application use cases without a WebView or operating-system integration.
mod contracts;
mod service;
mod storage;

pub use contracts::*;
pub use service::CoreService;
pub use storage::{Storage, SCHEMA_VERSION};
