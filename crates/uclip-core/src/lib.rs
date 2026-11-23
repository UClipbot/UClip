//! Application use cases without a WebView or operating-system integration.
mod collections;
mod contracts;
mod desktop;
pub mod extensions;
mod history;
mod keyboard;
mod replica;
mod service;
mod storage;

pub use contracts::*;
pub use desktop::{AuxiliaryWindow, AuxiliaryWindowSize, WindowPlacement};
pub use history::{ClipPayload, QuickPasteSlot, MAX_IMAGE_PIXELS, MAX_TEXT_BYTES};
pub use keyboard::*;
pub use replica::{
    FileReplacement, FileTicket, ReplicaBody, ReplicaEntry, ReplicaId, ReplicaStamp, ReplicaType,
};
pub use service::CoreService;
pub use storage::{Storage, SCHEMA_VERSION};
