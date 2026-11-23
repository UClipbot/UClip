//! Bounded native formats and checked publication. No business rule lives here.
use std::io::Cursor;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use uclip_core::MAX_TEXT_BYTES;
use uclip_core::{AppError, ClipPayload, ErrorCode, MAX_IMAGE_PIXELS};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::Context;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::ClipboardWrite;
#[cfg(target_os = "windows")]
pub use windows::Context;
pub const MAX_ENCODED_IMAGE: usize = 40 * 1024 * 1024;
fn error() -> AppError {
    AppError::new(ErrorCode::Clipboard)
}
fn invalid() -> AppError {
    AppError::new(ErrorCode::InvalidInput)
}
fn limit() -> AppError {
    AppError::new(ErrorCode::LimitExceeded)
}
fn check_length(length: usize, maximum: usize) -> Result<(), AppError> {
    if length > maximum {
        Err(limit())
    } else {
        Ok(())
    }
}
#[cfg(any(target_os = "macos", test))]
fn text(bytes: Vec<u8>) -> Result<Option<ClipPayload>, AppError> {
    check_length(bytes.len(), MAX_TEXT_BYTES)?;
    let value = String::from_utf8(bytes).map_err(|_| invalid())?;
    Ok((!value.is_empty()).then_some(ClipPayload::Text(value)))
}
pub fn decode_image(
    bytes: &[u8],
    format: Option<image::ImageFormat>,
) -> Result<ClipPayload, AppError> {
    check_length(bytes.len(), MAX_ENCODED_IMAGE)?;
    let reader = || -> Result<image::ImageReader<Cursor<&[u8]>>, AppError> {
        let mut r = image::ImageReader::new(Cursor::new(bytes));
        if let Some(format) = format {
            r.set_format(format);
        } else {
            r = r.with_guessed_format().map_err(|_| invalid())?;
        }
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_IMAGE_PIXELS as u32);
        limits.max_image_height = Some(MAX_IMAGE_PIXELS as u32);
        limits.max_alloc = Some(64 * 1024 * 1024);
        r.limits(limits);
        Ok(r)
    };
    // Header-only dimensions check must precede pixel allocation/decompression.
    let (width, height) = reader()?.into_dimensions().map_err(|e| {
        if matches!(e, image::ImageError::Limits(_)) {
            limit()
        } else {
            invalid()
        }
    })?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_IMAGE_PIXELS {
        return Err(limit());
    }
    let image = reader()?
        .decode()
        .map_err(|e| {
            if matches!(e, image::ImageError::Limits(_)) {
                limit()
            } else {
                invalid()
            }
        })?
        .into_rgba8();
    let payload = ClipPayload::Image {
        width,
        height,
        rgba: image.into_raw(),
    };
    payload.validate()?;
    Ok(payload)
}
pub fn png(width: u32, height: u32, rgba: Vec<u8>) -> Result<Vec<u8>, AppError> {
    let image = image::RgbaImage::from_raw(width, height, rgba).ok_or_else(invalid)?;
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|_| error())?;
    Ok(out.into_inner())
}
#[cfg(any(target_os = "windows", test))]
fn utf16_text(bytes: &[u8]) -> Result<Option<ClipPayload>, AppError> {
    check_length(bytes.len(), MAX_TEXT_BYTES * 2 + 64)?;
    let end = bytes
        .chunks_exact(2)
        .position(|b| b == [0, 0])
        .ok_or_else(invalid)?;
    let units = bytes[..end * 2]
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]));
    let mut out = String::with_capacity(end.min(MAX_TEXT_BYTES));
    for value in char::decode_utf16(units) {
        let value = value.map_err(|_| invalid())?;
        check_length(out.len() + value.len_utf8(), MAX_TEXT_BYTES)?;
        out.push(value);
    }
    Ok((!out.is_empty()).then_some(ClipPayload::Text(out)))
}
#[cfg(any(target_os = "windows", test))]
fn dib(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    // BITMAPV5HEADER, top-down BGRA with explicit alpha and sRGB masks.
    let mut data = vec![0u8; 124];
    for (offset, value) in [
        (0, 124u32),
        (4, width),
        (8, (-(height as i32)) as u32),
        (16, 3),
        (20, rgba.len() as u32),
        (40, 0x00ff0000),
        (44, 0x0000ff00),
        (48, 0x000000ff),
        (52, 0xff000000),
        (56, 0x73524742),
        (108, 4),
    ] {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    data[12..14].copy_from_slice(&1u16.to_le_bytes());
    data[14..16].copy_from_slice(&32u16.to_le_bytes());
    data.extend(rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]));
    data
}
#[cfg(any(target_os = "windows", test))]
fn decode_dib(data: Vec<u8>) -> Result<ClipPayload, AppError> {
    check_length(data.len(), MAX_ENCODED_IMAGE)?;
    if data.len() < 40 {
        return Err(invalid());
    }
    // Supply an explicit BMP pixel offset: packed DIB V4/V5 masks are inside
    // their header, whereas BITMAPINFOHEADER masks follow the header.
    let u32_at = |offset| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    let header = u32_at(0) as usize;
    if ![40, 52, 56, 108, 124].contains(&header) || header > data.len() {
        return Err(invalid());
    }
    let depth = u16::from_le_bytes([data[14], data[15]]);
    let compression = u32_at(16);
    if ![0, 3, 6].contains(&compression) {
        return Err(invalid());
    }
    let colors = u32_at(32) as usize;
    let colors = if colors == 0 && depth <= 8 {
        1usize << depth
    } else {
        colors
    };
    if colors > 256 {
        return Err(invalid());
    }
    let masks = if header == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let mut offset = header + masks + colors * 4;
    if header == 124 {
        let profile_offset = u32_at(112) as usize;
        let profile_size = u32_at(116) as usize;
        if profile_size != 0 {
            if profile_offset != offset {
                return Err(invalid());
            }
            offset = offset.checked_add(profile_size).ok_or_else(invalid)?;
        }
    }
    if offset > data.len() {
        return Err(invalid());
    }
    let mut bmp = Vec::with_capacity(data.len() + 14);
    bmp.extend(b"BM");
    bmp.extend(((data.len() + 14) as u32).to_le_bytes());
    bmp.extend([0u8; 4]);
    bmp.extend(((offset + 14) as u32).to_le_bytes());
    bmp.extend(data);
    use image::ImageDecoder;
    let mut decoder =
        image::codecs::bmp::BmpDecoder::new(Cursor::new(bmp)).map_err(|_| invalid())?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err(limit());
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    decoder.set_limits(limits).map_err(|_| limit())?;
    let image = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| invalid())?
        .into_rgba8();
    let payload = ClipPayload::Image {
        width,
        height,
        rgba: image.into_raw(),
    };
    payload.validate()?;
    Ok(payload)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_text_limits_preserve_unicode_and_reject_invalid_or_unterminated_input() {
        let value = " 你好\n😀 ";
        let bytes = value
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(
            utf16_text(&bytes).unwrap(),
            Some(ClipPayload::Text(value.into()))
        );
        assert!(utf16_text(&[65, 0]).is_err());
        assert!(utf16_text(&[0, 0xd8, 0, 0]).is_err());
        assert_eq!(utf16_text(&[0, 0]).unwrap(), None);
        assert!(check_length(MAX_TEXT_BYTES * 2 + 3, MAX_TEXT_BYTES * 2 + 2).is_err());
        assert!(text(vec![b'x'; MAX_TEXT_BYTES + 1]).is_err());
        assert!(text(vec![0xff]).is_err());
        let exact = "x".repeat(MAX_TEXT_BYTES);
        let raw = exact
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(utf16_text(&raw).unwrap(), Some(ClipPayload::Text(exact)));
        let over = "中".repeat(MAX_TEXT_BYTES / 3 + 1);
        let raw = over
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(utf16_text(&raw).is_err());
    }
    #[test]
    fn native_image_codecs_preserve_pixels_and_reject_oversized_headers() {
        let rgba = vec![12, 34, 56, 0, 100, 120, 140, 255];
        let expected = ClipPayload::Image {
            width: 2,
            height: 1,
            rgba: rgba.clone(),
        };
        assert_eq!(
            decode_image(&png(2, 1, rgba.clone()).unwrap(), None).unwrap(),
            expected
        );
        assert_eq!(decode_dib(dib(2, 1, &rgba)).unwrap(), expected);
        let mut oversized = dib(2, 1, &rgba);
        oversized[4..8].copy_from_slice(&((MAX_IMAGE_PIXELS + 1) as u32).to_le_bytes());
        assert!(decode_dib(oversized).is_err());
        assert!(decode_image(b"invalid", None).is_err());
        assert!(decode_dib(vec![0; 39]).is_err());
        let mut header = png(1, 1, vec![0, 0, 0, 255]).unwrap();
        header[16..20].copy_from_slice(&((MAX_IMAGE_PIXELS + 1) as u32).to_be_bytes());
        let mut crc = !0u32;
        for byte in &header[12..29] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        header[29..33].copy_from_slice(&(!crc).to_be_bytes());
        assert_eq!(
            decode_image(&header, None).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
        // Ordinary 24-bit bottom-up DIB with padded scanline, no file header.
        let mut rgb = vec![0u8; 40];
        for (at, value) in [(0, 40u32), (4, 2), (8, 1), (20, 8)] {
            rgb[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        rgb[12..14].copy_from_slice(&1u16.to_le_bytes());
        rgb[14..16].copy_from_slice(&24u16.to_le_bytes());
        rgb.extend([56, 34, 12, 140, 120, 100, 0, 0]);
        assert_eq!(
            decode_dib(rgb).unwrap(),
            ClipPayload::Image {
                width: 2,
                height: 1,
                rgba: vec![12, 34, 56, 255, 100, 120, 140, 255]
            }
        );
    }
}
