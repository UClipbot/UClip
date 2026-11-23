use uclip_core::{AppError, ClipPayload};
use uclip_runtime::clipboard::{self, Context, CLI_MARKER};

#[cfg(not(target_os = "linux"))]
pub fn publish(payload: ClipPayload) -> Result<(), AppError> {
    let context = Context::new()?;
    clipboard::write(&context, payload, CLI_MARKER, b"1")?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub use x11::{hold_selection, publish};

#[cfg(target_os = "linux")]
mod x11 {
    use std::{
        io::{self, BufRead, BufReader, Read, Write},
        os::unix::process::CommandExt,
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };

    use serde::{Deserialize, Serialize};
    use uclip_core::ErrorCode;
    use uclip_runtime::clipboard_x11::Reader;

    use super::*;

    const MAX_PACKET: usize = 52 * 1024 * 1024;

    fn error() -> AppError {
        AppError::new(ErrorCode::Clipboard)
    }

    #[derive(Serialize, Deserialize)]
    enum Payload {
        Text(String),
        Image {
            width: u32,
            height: u32,
            #[serde(with = "serde_bytes")]
            rgba: Vec<u8>,
        },
        Files(Vec<String>),
    }

    fn write_packet(payload: ClipPayload, mut pipe: impl Write) -> Result<(), AppError> {
        let payload = match payload {
            ClipPayload::Text(text) => Payload::Text(text),
            ClipPayload::Image {
                width,
                height,
                rgba,
            } => Payload::Image {
                width,
                height,
                rgba,
            },
            ClipPayload::Files(paths) => Payload::Files(paths),
        };
        rmp_serde::encode::write(&mut pipe, &payload).map_err(|_| error())
    }

    pub fn publish(payload: ClipPayload) -> Result<(), AppError> {
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value == "wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var_os("DISPLAY").is_none()
        {
            return Err(AppError::new(ErrorCode::Restricted));
        }
        payload.validate()?;
        // Image publication also encodes the PNG before taking ownership.
        let timeout = if matches!(payload, ClipPayload::Image { .. }) {
            Duration::from_secs(15)
        } else {
            Duration::from_secs(5)
        };
        let executable = std::env::current_exe().map_err(|_| error())?;
        let mut child = Command::new(executable)
            .arg("__clipboard-owner")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| error())?;
        let result = (|| {
            let pipe = child.stdin.take().ok_or_else(error)?;
            let response = child.stderr.take().ok_or_else(error)?;
            let (completed, receipt) = mpsc::sync_channel(1);
            std::thread::Builder::new()
                .name("uclip-clipboard-publication".into())
                .spawn(move || {
                    let result = (|| {
                        write_packet(payload, pipe)?;
                        let mut line = Vec::new();
                        BufReader::new(response.take(256))
                            .read_until(b'\n', &mut line)
                            .map_err(|_| error())?;
                        serde_json::from_slice::<Result<(), ErrorCode>>(&line)
                            .map_err(|_| error())?
                            .map_err(AppError::new)
                    })();
                    let _ = completed.send(result);
                })
                .map_err(|_| error())?;
            receipt.recv_timeout(timeout).map_err(|_| error())?
        })();
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result
    }

    fn open_selection() -> Result<(Context, Reader, u32), AppError> {
        let packet = crate::history::read_bounded(io::stdin().lock(), MAX_PACKET)?;
        let mut decode = rmp_serde::Deserializer::new(packet.as_slice());
        decode.set_max_depth(8);
        let payload = match Payload::deserialize(&mut decode).map_err(|_| crate::invalid())? {
            Payload::Text(text) => ClipPayload::Text(text),
            Payload::Image {
                width,
                height,
                rgba,
            } => ClipPayload::Image {
                width,
                height,
                rgba,
            },
            Payload::Files(paths) => ClipPayload::Files(paths),
        };
        let reader = Reader::new()?;
        let context = Context::new()?;
        let marker = uuid::Uuid::new_v4().to_string();
        clipboard::write(&context, payload, CLI_MARKER, marker.as_bytes())?;
        // A later copy may have already replaced our selection.
        let owner = reader.owner()?;
        if owner == 0
            || reader.buffer(CLI_MARKER, marker.len())? != marker.as_bytes()
            || reader.owner()? != owner
        {
            return Err(error());
        }
        Ok((context, reader, owner))
    }

    pub fn hold_selection() -> Result<(), AppError> {
        let selection = open_selection();
        let receipt = selection.as_ref().map(|_| ()).map_err(|error| error.code);
        let mut acknowledgment = io::stderr().lock();
        serde_json::to_writer(&mut acknowledgment, &receipt).map_err(|_| error())?;
        writeln!(acknowledgment).map_err(|_| error())?;
        drop(acknowledgment);
        let (_context, reader, owner) = selection?;
        while reader.owner().is_ok_and(|current| current == owner) {
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    }
}
