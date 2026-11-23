use std::{
    fs::File,
    io::{self, Read, Write},
    path::PathBuf,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use clap::Args;
use serde::Serialize;
use uclip_core::{
    AppError, ClipKind, ClipPayload, CoreService, ErrorCode, HistoryItem, HistoryPage,
    HistoryQuery, MAX_TEXT_BYTES,
};
use uclip_runtime::clipboard_native::{decode_image, png, MAX_ENCODED_IMAGE};

#[derive(Args)]
pub struct Get {
    /// Record UUID or latest. Defaults to the first record in the selected scope.
    #[arg(conflicts_with_all = ["position", "index"])]
    id: Option<String>,
    /// One-based position in desktop list order.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=25_000))]
    position: Option<u32>,
    /// Zero-based position in desktop list order.
    #[arg(short, long, conflicts_with = "position", value_parser = clap::value_parser!(u32).range(..25_000))]
    index: Option<u32>,
    /// Select an active collection by name or UUID.
    #[arg(long)]
    collection: Option<String>,
    /// Also write the record to the system clipboard.
    #[arg(short, long)]
    copy: bool,
    /// Output the original bytes without a trailing newline.
    #[arg(short, long)]
    raw: bool,
}

#[derive(Args)]
pub struct Listing {
    /// Select an active collection by name or UUID.
    #[arg(long)]
    collection: Option<String>,
    /// Maximum records to return.
    #[arg(short, long, visible_short_alias = 'n', value_parser = clap::value_parser!(u32).range(1..=1000))]
    limit: Option<u32>,
    /// Output one JSON history page.
    #[arg(long, conflicts_with = "full")]
    json: bool,
    /// Output JSON Lines with full content. Images are base64 PNG.
    #[arg(short, long)]
    full: bool,
}

#[derive(Args)]
pub struct Input {
    /// Literal text. With no source, read UTF-8 text from stdin.
    #[arg(conflicts_with_all = ["file", "image", "reference"])]
    text: Option<String>,
    /// Read UTF-8 text from a file, or - for stdin.
    #[arg(long, conflicts_with_all = ["image", "reference"])]
    file: Option<PathBuf>,
    /// Read an image from a file, or - for stdin.
    #[arg(long, conflicts_with_all = ["reference"])]
    image: Option<PathBuf>,
    /// Reference a file or directory without reading its contents. Repeat to add more paths.
    #[arg(long)]
    reference: Vec<PathBuf>,
}

#[derive(Args)]
pub struct Save {
    #[command(flatten)]
    input: Input,
    /// Save into an existing collection by name or UUID.
    #[arg(long)]
    collection: Option<String>,
    /// Also copy the saved content.
    #[arg(long)]
    copy: bool,
}

#[derive(Args)]
pub struct Copy {
    #[command(flatten)]
    pub input: Input,
    /// Copy an existing record by UUID.
    #[arg(long, conflicts_with_all = ["save", "collection", "text", "file", "image", "reference"])]
    pub id: Option<String>,
    /// Save the input to history as well.
    #[arg(long)]
    pub save: bool,
    /// Save into an existing collection by name or UUID.
    #[arg(long, requires = "save")]
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
    match selection {
        Some(selection) => core
            .collections()
            .await?
            .items
            .into_iter()
            .find(|collection| collection.id == selection || collection.name == selection)
            .map(|collection| Some(collection.id))
            .ok_or_else(|| AppError::new(ErrorCode::NotFound)),
        None => Ok(None),
    }
}

async fn page(
    core: &CoreService,
    collection: Option<String>,
    offset: u32,
    count: u32,
    query: String,
    regex: bool,
    kind: Option<ClipKind>,
) -> Result<HistoryPage, AppError> {
    let query = HistoryQuery {
        query,
        regex,
        kind,
        pinned_only: false,
        collection_id: collection_id(core, collection).await?,
        offset,
        expected_revision: Some(core.history_revision().await?),
    };
    core.history_window(query, false, Some(count)).await
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
            let page = page(
                core,
                args.collection,
                args.index.unwrap_or(args.position.unwrap_or(1) - 1),
                1,
                String::new(),
                false,
                None,
            )
            .await?;
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
        ClipPayload::Text(text) => output(text.as_bytes(), !args.raw && !text.ends_with('\n')),
        ClipPayload::Files(paths) => print_files(&paths, !args.raw),
        ClipPayload::Image {
            width,
            height,
            rgba,
        } => output(&png(width, height, rgba)?, false),
    }
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
    let page = page(
        core,
        args.collection,
        0,
        args.limit.unwrap_or(default_limit),
        query,
        regex,
        kind,
    )
    .await?;
    if args.json {
        return crate::print_json(&page);
    }
    for item in page.items {
        if args.full {
            let payload = core
                .read_clip_in_history(item.id.clone(), Some(page.revision.clone()))
                .await?;
            crate::print_json(&FullItem {
                item,
                content: payload.try_into()?,
            })?;
        } else {
            let preview = serde_json::to_string(&item.preview).map_err(|_| crate::unavailable())?;
            let kind = match item.kind {
                ClipKind::Text => "text",
                ClipKind::Image => "image",
                ClipKind::Files => "files",
            };
            let line = format!("{}\t{kind}\t{preview}", item.id);
            output(line.as_bytes(), true)?;
        }
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
        eprintln!("uclip: clipboard written; usage could not be saved");
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
    let id = core
        .create_clip(payload, collection_id(core, collection).await?)
        .await?;
    output(id.as_bytes(), true)?;
    if copy {
        let payload = core
            .clipboard_payload(vec![id.clone()], None, None, Vec::new())
            .await?;
        if let Err(error) = publish_record(core, id, payload).await {
            eprintln!("uclip: record saved");
            return Err(error);
        }
    }
    Ok(())
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
