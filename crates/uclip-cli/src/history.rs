use std::{
    fs::File,
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use clap::Args;
use serde::Serialize;
use uclip_core::{
    AppError, ClipKind, ClipPayload, CoreService, ErrorCode, HistoryItem, HistoryQuery,
    MAX_TEXT_BYTES,
};
use uclip_runtime::clipboard_native::{decode_image, png, MAX_ENCODED_IMAGE};

use crate::i18n::{messages, text};

#[derive(Args)]
pub struct Get {
    #[arg(help = text(messages::RECORD_ID), conflicts_with_all = ["position", "index"])]
    id: Option<String>,
    #[arg(help = text(messages::POSITION), long, value_parser = clap::value_parser!(u32).range(1..=25_000))]
    position: Option<u32>,
    #[arg(help = text(messages::INDEX), short, long, conflicts_with = "position", value_parser = clap::value_parser!(u32).range(..25_000))]
    index: Option<u32>,
    #[arg(help = text(messages::SELECT_COLLECTION), long)]
    collection: Option<String>,
    #[arg(help = text(messages::GET_COPY), short, long)]
    copy: bool,
    #[arg(help = text(messages::RAW), short, long)]
    raw: bool,
}

#[derive(Args)]
pub struct Listing {
    #[arg(help = text(messages::SELECT_COLLECTION), long)]
    collection: Option<String>,
    #[arg(help = text(messages::LIMIT), short, long, visible_short_alias = 'n', value_parser = clap::value_parser!(u32).range(1..=1000))]
    limit: Option<u32>,
    #[arg(help = text(messages::JSON), long, conflicts_with = "full")]
    json: bool,
    #[arg(help = text(messages::FULL), short, long)]
    full: bool,
}

#[derive(Args)]
pub struct Input {
    #[arg(help = text(messages::TEXT), conflicts_with_all = ["file", "image", "reference"])]
    text: Option<String>,
    #[arg(help = text(messages::FILE), long, conflicts_with_all = ["image", "reference"])]
    file: Option<PathBuf>,
    #[arg(help = text(messages::IMAGE), long, conflicts_with_all = ["reference"])]
    image: Option<PathBuf>,
    #[arg(help = text(messages::REFERENCE), long)]
    reference: Vec<PathBuf>,
}

#[derive(Args)]
pub struct Save {
    #[command(flatten)]
    input: Input,
    #[arg(help = text(messages::SAVE_COLLECTION), long)]
    collection: Option<String>,
    #[arg(help = text(messages::SAVE_COPY), long)]
    copy: bool,
}

#[derive(Args)]
pub struct Copy {
    #[command(flatten)]
    pub input: Input,
    #[arg(help = text(messages::COPY_ID), long, conflicts_with_all = ["save", "collection", "text", "file", "image", "reference"])]
    pub id: Option<String>,
    #[arg(help = text(messages::COPY_SAVE), long)]
    pub save: bool,
    #[arg(help = text(messages::SAVE_COLLECTION), long, requires = "save")]
    collection: Option<String>,
}

impl Input {
    pub fn read(self) -> Result<ClipPayload, AppError> {
        let payload = if let Some(image) = self.image {
            decode_image(&read_input(Some(image), MAX_ENCODED_IMAGE)?, None)?
        } else if !self.reference.is_empty() {
            let paths = self
                .reference
                .into_iter()
                .map(|path| {
                    std::path::absolute(path)
                        .map_err(|_| crate::invalid())?
                        .into_os_string()
                        .into_string()
                        .map_err(|_| crate::invalid())
                })
                .collect::<Result<Vec<_>, _>>()?;
            ClipPayload::Files(paths)
        } else if let Some(text) = self.text {
            ClipPayload::Text(text)
        } else {
            ClipPayload::Text(
                String::from_utf8(read_input(self.file, MAX_TEXT_BYTES)?)
                    .map_err(|_| crate::invalid())?,
            )
        };
        payload.validate()?;
        Ok(payload)
    }
}

fn read_input(path: Option<PathBuf>, limit: usize) -> Result<Vec<u8>, AppError> {
    if let Some(path) = path.filter(|path| path.as_os_str() != "-") {
        let file_error = |error: io::Error| {
            if error.kind() == io::ErrorKind::NotFound {
                AppError::new(ErrorCode::NotFound)
            } else {
                crate::storage()
            }
        };
        if !std::fs::metadata(&path).map_err(file_error)?.is_file() {
            return Err(crate::invalid());
        }
        let file = File::open(path).map_err(file_error)?;
        if !file.metadata().map_err(|_| crate::storage())?.is_file() {
            return Err(crate::invalid());
        }
        read_bounded(file, limit)
    } else {
        read_bounded(io::stdin().lock(), limit)
    }
}

pub fn read_bounded(reader: impl Read, limit: usize) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| crate::unavailable())?;
    if bytes.len() > limit {
        return Err(AppError::new(ErrorCode::LimitExceeded));
    }
    Ok(bytes)
}

pub fn output(bytes: &[u8], newline: bool) -> Result<(), AppError> {
    let mut out = io::stdout().lock();
    out.write_all(bytes).map_err(|_| crate::unavailable())?;
    if newline {
        out.write_all(b"\n").map_err(|_| crate::unavailable())?;
    }
    out.flush().map_err(|_| crate::unavailable())
}

async fn collection_id(
    core: &CoreService,
    selection: Option<String>,
) -> Result<Option<String>, AppError> {
    let Some(selection) = selection else {
        return Ok(None);
    };
    let items = core.collections().await?.items;
    items
        .iter()
        .find(|collection| collection.id == selection)
        .or_else(|| items.iter().find(|collection| collection.name == selection))
        .map(|collection| Some(collection.id.clone()))
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))
}

fn id(value: String) -> Result<String, AppError> {
    uuid::Uuid::parse_str(&value)
        .map(|value| value.to_string())
        .map_err(|_| crate::invalid())
}

pub async fn get(core: &CoreService, args: Get) -> Result<(), AppError> {
    let (id, revision) = match args.id.filter(|value| value != "latest") {
        Some(value) if args.collection.is_none() => (id(value)?, None),
        Some(_) => return Err(crate::invalid()),
        None => {
            let query = HistoryQuery {
                query: String::new(),
                regex: false,
                kind: None,
                pinned_only: false,
                collection_id: collection_id(core, args.collection).await?,
                offset: args.index.unwrap_or(args.position.unwrap_or(1) - 1),
                expected_revision: Some(core.history_revision().await?),
            };
            let page = core.history_window(query, false, Some(1)).await?;
            let clip = page
                .items
                .into_iter()
                .next()
                .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
            (clip.id, Some(page.revision))
        }
    };
    let payload = core
        .read_clip_in_history(id.clone(), revision.clone())
        .await?;
    if args.copy {
        let to_copy = core
            .clipboard_payload(vec![id.clone()], revision, None, Vec::new())
            .await?;
        if to_copy != payload {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        publish_record(core, id, to_copy).await?;
    }
    match payload {
        ClipPayload::Text(text) => {
            let text = if !args.raw && io::stdout().is_terminal() {
                terminal_text(&text)
            } else {
                text
            };
            output(text.as_bytes(), !args.raw && !text.ends_with('\n'))
        }
        ClipPayload::Files(paths) => print_files(&paths, !args.raw),
        ClipPayload::Image {
            width,
            height,
            rgba,
        } => output(&png(width, height, rgba)?, false),
    }
}

fn terminal_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' && characters.peek() == Some(&'\n') {
            continue;
        }
        if character.is_control() && !matches!(character, '\n' | '\t') {
            escaped.extend(character.escape_unicode());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn print_files(paths: &[String], newline: bool) -> Result<(), AppError> {
    let bytes = serde_json::to_vec(paths).map_err(|_| crate::unavailable())?;
    output(&bytes, newline)
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Body {
    Text { text: String },
    Files { paths: Vec<String> },
    Image { png_base64: String },
}

impl TryFrom<ClipPayload> for Body {
    type Error = AppError;
    fn try_from(payload: ClipPayload) -> Result<Self, AppError> {
        Ok(match payload {
            ClipPayload::Text(text) => Self::Text { text },
            ClipPayload::Files(paths) => Self::Files { paths },
            ClipPayload::Image {
                width,
                height,
                rgba,
            } => Self::Image {
                png_base64: STANDARD.encode(png(width, height, rgba)?),
            },
        })
    }
}

#[derive(Serialize)]
struct FullItem {
    item: HistoryItem,
    content: Body,
}

pub async fn list(
    core: &CoreService,
    args: Listing,
    query: String,
    regex: bool,
    kind: Option<ClipKind>,
) -> Result<(), AppError> {
    let default_limit = if kind.is_some() { 20 } else { 10 };
    let limit = args.limit.unwrap_or(default_limit);
    let mut query = HistoryQuery {
        query,
        regex,
        kind,
        pinned_only: false,
        collection_id: collection_id(core, args.collection).await?,
        offset: 0,
        expected_revision: None,
    };
    if args.full {
        for (item, payload) in core.history_content(query, limit).await? {
            crate::print_json(&FullItem {
                item,
                content: payload.try_into()?,
            })?;
        }
        return Ok(());
    }
    query.expected_revision = Some(core.history_revision().await?);
    let page = core.history_window(query, false, Some(limit)).await?;
    if args.json {
        return crate::print_json(&page);
    }
    for item in page.items {
        let preview = serde_json::to_string(&item.preview).map_err(|_| crate::unavailable())?;
        let kind = match item.kind {
            ClipKind::Text => "text",
            ClipKind::Image => "image",
            ClipKind::Files => "files",
        };
        let line = format!("{}\t{kind}\t{preview}", item.id);
        output(line.as_bytes(), true)?;
    }
    Ok(())
}

async fn publish_record(
    core: &CoreService,
    id: String,
    payload: ClipPayload,
) -> Result<(), AppError> {
    crate::clipboard::publish(payload)?;
    if let Err(error) = core.mark_used(id).await {
        crate::i18n::notice(messages::CLIPBOARD_USAGE_FAILED);
        return Err(error);
    }
    Ok(())
}

async fn save_input(
    core: &CoreService,
    input: Input,
    collection: Option<String>,
    copy: bool,
) -> Result<(), AppError> {
    let payload = input.read()?;
    let clip = core
        .create_clip_version(payload, collection_id(core, collection).await?)
        .await?;
    let result = async {
        output(clip.id.as_bytes(), true)?;
        if copy {
            let payload = core
                .clipboard_payload(
                    vec![clip.id.clone()],
                    None,
                    Some(clip.expected_revision),
                    Vec::new(),
                )
                .await?;
            publish_record(core, clip.id, payload).await?;
        }
        Ok(())
    }
    .await;
    result.inspect_err(|_| crate::i18n::notice(messages::RECORD_SAVED))
}

pub async fn save(core: &CoreService, args: Save) -> Result<(), AppError> {
    save_input(core, args.input, args.collection, args.copy).await
}

pub async fn copy(core: &CoreService, args: Copy) -> Result<(), AppError> {
    if let Some(id_value) = args.id {
        let id = id(id_value)?;
        let payload = core
            .clipboard_payload(vec![id.clone()], None, None, Vec::new())
            .await?;
        return publish_record(core, id, payload).await;
    }
    save_input(core, args.input, args.collection, true).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn terminal_text_escapes_controls_and_preserves_readable_whitespace() {
        assert_eq!(
            super::terminal_text("\u{7b14}\tline\r\n\r\x1b\u{9b}\x7f\0"),
            "\u{7b14}\tline\n\\u{d}\\u{1b}\\u{9b}\\u{7f}\\u{0}"
        );
    }
}
