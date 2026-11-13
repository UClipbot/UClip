use super::*;
use objc2::{rc::autoreleasepool, runtime::ProtocolObject};
use objc2_app_kit::{NSPasteboard, NSPasteboardItem};
use objc2_foundation::{NSArray, NSData, NSString};

pub struct Context;
impl Context {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self)
    }
    pub fn read(&self, marker: &[u8]) -> Result<Option<ClipPayload>, AppError> {
        autoreleasepool(|_| {
            let board = NSPasteboard::generalPasteboard();
            let types = board.types();
            let Some(types) = types else {
                return Ok(None);
            };
            check_length(types.len(), 256)?;
            let names = types
                .iter()
                .map(|name| {
                    check_length(name.len(), 256)?;
                    Ok(name.to_string())
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            if crate::clipboard::sensitive(&names, |name| buffer(&board, name, 16)) {
                return Ok(None);
            }
            if names.iter().any(|n| n == crate::clipboard::MARKER)
                && buffer(&board, crate::clipboard::MARKER, 64).is_ok_and(|v| v == marker)
            {
                return Ok(None);
            }
            let has = |name: &str| names.iter().any(|n| n == name);
            if has("public.file-url") {
                let items = board.pasteboardItems().ok_or_else(error)?;
                check_length(items.len(), 256)?;
                let mut paths = Vec::new();
                for item in items {
                    // Marker/other representations must be on the same file item.
                    let data = item
                        .dataForType(&NSString::from_str("public.file-url"))
                        .ok_or_else(error)?;
                    check_length(data.len(), 128 * 1024)?;
                    paths.push(String::from_utf8(data.to_vec()).map_err(|_| invalid())?);
                }
                return Ok(Some(ClipPayload::Files(crate::clipboard::normalize_files(
                    paths,
                )?)));
            }
            for (name, format) in [
                ("public.png", image::ImageFormat::Png),
                ("public.tiff", image::ImageFormat::Tiff),
            ] {
                if has(name) {
                    return decode_image(&buffer(&board, name, MAX_ENCODED_IMAGE)?, Some(format))
                        .map(Some);
                }
            }
            if has("public.utf8-plain-text") {
                return text(buffer(&board, "public.utf8-plain-text", MAX_TEXT_BYTES)?);
            }
            Ok(None)
        })
    }
    pub fn write(&self, payload: ClipPayload, marker: &[u8]) -> Result<(), AppError> {
        autoreleasepool(|_| {
            let representations = match payload {
                ClipPayload::Text(value) => vec![("public.utf8-plain-text", value.into_bytes())],
                ClipPayload::Image {
                    width,
                    height,
                    rgba,
                } => vec![("public.png", png(width, height, rgba)?)],
                ClipPayload::Files(paths) => paths
                    .into_iter()
                    .map(|path| {
                        let uri = url::Url::from_file_path(path).map_err(|_| invalid())?;
                        Ok(("public.file-url", uri.to_string().into_bytes()))
                    })
                    .collect::<Result<Vec<_>, AppError>>()?,
            };
            // Prepare all items first, checking every format write. File URLs and
            // marker share each item; one writeObjects call publishes the entire list.
            let mut items = Vec::with_capacity(representations.len());
            for (name, bytes) in representations {
                let item = NSPasteboardItem::new();
                if !item.setData_forType(&NSData::with_bytes(&bytes), &NSString::from_str(name))
                    || !item.setData_forType(
                        &NSData::with_bytes(marker),
                        &NSString::from_str(crate::clipboard::MARKER),
                    )
                {
                    return Err(error());
                }
                items.push(ProtocolObject::from_retained(item));
            }
            let objects = NSArray::from_retained_slice(&items);
            let board = NSPasteboard::generalPasteboard();
            board.clearContents();
            if !board.writeObjects(&objects) {
                return Err(error());
            }
            Ok(())
        })
    }
}
fn buffer(board: &NSPasteboard, name: &str, maximum: usize) -> Result<Vec<u8>, AppError> {
    let data = board
        .dataForType(&NSString::from_str(name))
        .ok_or_else(error)?;
    check_length(data.len(), maximum)?;
    Ok(data.to_vec())
}
