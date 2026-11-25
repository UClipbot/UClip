use std::{
    cell::Cell,
    fs::File,
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use clap::Args;
use serde::{Deserialize, Serialize};
use uclip_core::{
    archive::{export_history, import_history, ArchiveClip, ArchiveCollection, ArchiveItem},
    AppError, ClipPayload, ErrorCode,
};
use uclip_runtime::clipboard_native::{decode_image, png, MAX_ENCODED_IMAGE};

use crate::i18n::{messages, text};

const VERSION: u32 = 1;
const MAX_FRAME: u64 = 64 * 1024 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_END: &[u8] = b"\0\0\0\0IEND\xaeB\x60\x82";

#[derive(Args)]
pub struct Import {
    #[arg(long, help = text(messages::ARCHIVE_INPUT))]
    input: Option<PathBuf>,
    #[arg(long, help = text(messages::IMPORT_APPLY))]
    apply: bool,
    #[arg(long, help = text(messages::IMPORT_SKIP_INVALID))]
    skip_invalid: bool,
}

#[derive(Args)]
pub struct Export {
    #[arg(long, help = text(messages::ARCHIVE_OUTPUT))]
    output: Option<PathBuf>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Frame {
    Header {
        version: u32,
    },
    Collection(ArchiveCollection),
    Clip {
        #[serde(flatten)]
        clip: ArchiveClip,
        #[serde(flatten)]
        payload: Payload,
    },
    End,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", content = "content", rename_all = "snake_case")]
enum Payload {
    Text(String),
    Image(String),
    Files(Vec<String>),
}

impl Payload {
    fn decode(self) -> Result<ClipPayload, AppError> {
        let payload = match self {
            Self::Text(text) => ClipPayload::Text(text),
            Self::Files(paths) => ClipPayload::Files(paths),
            Self::Image(encoded) => {
                if encoded.len() > MAX_ENCODED_IMAGE.div_ceil(3) * 4 {
                    return Err(AppError::new(ErrorCode::LimitExceeded));
                }
                let data = STANDARD.decode(encoded).map_err(|_| crate::invalid())?;
                if !data.starts_with(PNG_SIGNATURE) || !data.ends_with(PNG_END) {
                    return Err(crate::invalid());
                }
                decode_image(&data, None)?
            }
        };
        payload.validate()?;
        Ok(payload)
    }

    fn encode(payload: ClipPayload) -> Result<Self, AppError> {
        Ok(match payload {
            ClipPayload::Text(text) => Self::Text(text),
            ClipPayload::Files(paths) => Self::Files(paths),
            ClipPayload::Image {
                width,
                height,
                rgba,
            } => Self::Image(STANDARD.encode(png(width, height, rgba)?)),
        })
    }
}

fn read_frame(reader: &mut impl BufRead, buffer: &mut Vec<u8>) -> Result<Frame, AppError> {
    buffer.clear();
    let bytes = reader
        .take(MAX_FRAME + 1)
        .read_until(b'\n', buffer)
        .map_err(|_| crate::unavailable())?;
    if bytes as u64 > MAX_FRAME {
        return Err(AppError::new(ErrorCode::LimitExceeded));
    }
    serde_json::from_slice(buffer).map_err(|_| crate::invalid())
}

pub fn import(path: &Path, args: Import) -> Result<(), AppError> {
    let stdin = io::stdin();
    let file = args
        .input
        .map(File::open)
        .transpose()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => AppError::new(ErrorCode::NotFound),
            _ => crate::unavailable(),
        })?;
    let mut reader: Box<dyn BufRead> = match file {
        Some(file) => Box::new(BufReader::new(file)),
        None => Box::new(stdin.lock()),
    };
    let mut buffer = Vec::new();
    if !matches!(
        read_frame(&mut reader, &mut buffer)?,
        Frame::Header { version: VERSION }
    ) {
        return Err(crate::invalid());
    }
    let skipped = Cell::new(0u64);
    let entries = std::iter::from_fn(|| loop {
        let entry = match read_frame(&mut reader, &mut buffer) {
            Ok(Frame::Collection(collection)) => Ok(ArchiveItem::Collection(collection)),
            Ok(Frame::Clip { clip, payload }) => match payload.decode() {
                Ok(payload) => Ok(ArchiveItem::Clip(clip, payload)),
                Err(error) if args.skip_invalid => {
                    eprintln!(
                        "uclip: {} {} ({})",
                        text(messages::ARCHIVE_SKIPPED),
                        clip.id,
                        crate::i18n::error_name(error.code)
                    );
                    skipped.set(skipped.get() + 1);
                    continue;
                }
                Err(error) => Err(error),
            },
            Ok(Frame::End) => match reader.fill_buf() {
                Ok([]) => return None,
                Ok(_) => Err(crate::invalid()),
                Err(_) => Err(crate::unavailable()),
            },
            Ok(Frame::Header { .. }) => Err(crate::invalid()),
            Err(error) => Err(error),
        };
        return Some(entry);
    });
    let report = import_history(path, args.apply, entries)?;
    crate::print_json(&serde_json::json!({
        "mode": if args.apply { "apply" } else { "preview" },
        "skipped_invalid": skipped.get(),
        "history": report,
    }))
}

fn write_frame(writer: &mut impl Write, frame: &Frame) -> Result<(), AppError> {
    serde_json::to_writer(&mut *writer, frame).map_err(|_| crate::unavailable())?;
    writer.write_all(b"\n").map_err(|_| crate::unavailable())
}

fn write_archive(path: &Path, mut writer: impl Write) -> Result<(), AppError> {
    write_frame(&mut writer, &Frame::Header { version: VERSION })?;
    export_history(path, |item| {
        let frame = match item {
            ArchiveItem::Collection(collection) => Frame::Collection(collection),
            ArchiveItem::Clip(clip, payload) => Frame::Clip {
                clip,
                payload: Payload::encode(payload)?,
            },
        };
        write_frame(&mut writer, &frame)
    })?;
    write_frame(&mut writer, &Frame::End)?;
    writer.flush().map_err(|_| crate::unavailable())
}

pub fn export(path: &Path, args: Export) -> Result<(), AppError> {
    let Some(output) = args.output else {
        return write_archive(path, BufWriter::new(io::stdout().lock()));
    };
    let name = output.file_name().ok_or_else(crate::invalid)?;
    let name = name.to_string_lossy();
    #[cfg(windows)]
    let name = name
        .split(':')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.']);
    // An archive must not occupy a database filename or one of its sidecars.
    if matches!(
        name.to_ascii_lowercase().as_str(),
        "foundation.sqlite3"
            | "foundation.sqlite3-wal"
            | "foundation.sqlite3-shm"
            | "foundation.sqlite3-journal"
    ) {
        return Err(crate::invalid());
    }
    let directory = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file = tempfile::NamedTempFile::new_in(directory).map_err(|_| crate::unavailable())?;
    write_archive(path, BufWriter::new(file.as_file()))?;
    file.as_file()
        .sync_all()
        .map_err(|_| crate::unavailable())?;
    file.persist_noclobber(output)
        .map_err(|error| match error.error.kind() {
            io::ErrorKind::AlreadyExists => AppError::new(ErrorCode::Conflict),
            _ => crate::unavailable(),
        })?;
    Ok(())
}
