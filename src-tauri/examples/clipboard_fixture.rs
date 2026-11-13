//! Synthetic native source/reader for isolated-display integration tests. Never used by the app.
use clipboard_rs::{
    common::RustImage, Clipboard, ClipboardContent, ClipboardContext, RustImageData,
};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
fn run(
    context: &ClipboardContext,
    value: Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let operation = value["op"].as_str().ok_or("op")?;
    if operation == "read" {
        let kind = value["kind"].as_str().ok_or("kind")?;
        return Ok(match kind {
            "text" => json!({"text":context.get_text()?}),
            "files" => json!({"files":context.get_files()?}),
            "image_digest" => {
                let image = context.get_image()?;
                let (width, height) = image.get_size();
                let fingerprint = uclip_core::ClipPayload::Image {
                    width,
                    height,
                    rgba: image.to_rgba8()?.into_raw(),
                }
                .fingerprint();
                json!({"width":width,"height":height,"fingerprint":fingerprint})
            }
            "image" => {
                let image = context.get_image()?;
                let (w, h) = image.get_size();
                json!({"width":w,"height":h,"rgba":image.to_rgba8()?.into_raw()})
            }
            _ => return Err("kind".into()),
        });
    }
    let kind = value["kind"].as_str().ok_or("kind")?;
    let mut data = vec![match kind {
        "text" => ClipboardContent::Text(value["text"].as_str().ok_or("text")?.into()),
        "files" => ClipboardContent::Files(
            value["files"]
                .as_array()
                .ok_or("files")?
                .iter()
                .map(|v| {
                    let p = v.as_str().ok_or("path")?;
                    url::Url::from_file_path(p)
                        .map(|u| u.to_string())
                        .map_err(|_| "path")
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        "image" => {
            let width = value["width"].as_u64().ok_or("width")? as u32;
            let height = value["height"].as_u64().ok_or("height")? as u32;
            let rgba = if value["pattern"].as_bool() == Some(true) {
                [30u8, 70, 100, 128].repeat((width * height) as usize)
            } else {
                value["rgba"]
                    .as_array()
                    .ok_or("rgba")?
                    .iter()
                    .map(|v| v.as_u64().unwrap() as u8)
                    .collect()
            };
            ClipboardContent::Image(RustImageData::from_dynamic_image(
                image::DynamicImage::ImageRgba8(
                    image::RgbaImage::from_raw(width, height, rgba).ok_or("image")?,
                ),
            ))
        }
        _ => return Err("kind".into()),
    }];
    if value["cut"].as_bool() == Some(true) {
        let uris = value["files"]
            .as_array()
            .ok_or("files")?
            .iter()
            .map(|v| {
                url::Url::from_file_path(v.as_str().unwrap())
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        data = vec![
            ClipboardContent::Other("text/uri-list".into(), uris.join("\r\n").into_bytes()),
            ClipboardContent::Other(
                "x-special/gnome-copied-files".into(),
                format!("cut\n{}", uris.join("\n")).into_bytes(),
            ),
        ];
    }
    if value["textAlternative"].as_bool() == Some(true) {
        data.push(ClipboardContent::Text("image text alternative".into()));
    }
    if value["sensitive"].as_bool() == Some(true) {
        data.push(ClipboardContent::Other(
            "org.nspasteboard.ConcealedType".into(),
            vec![1],
        ));
    }
    context.set(data)?;
    Ok(json!({"set":true}))
}
fn main() {
    #[cfg(target_os = "linux")]
    if std::env::args().any(|a| a == "--shortcut-fixture") {
        shortcut_fixture().expect("isolated shortcut fixture");
        return;
    }
    let context = ClipboardContext::new().expect("isolated clipboard fixture");
    for line in io::stdin().lock().lines() {
        let result = line
            .map_err(|_| ())
            .and_then(|line| serde_json::from_str(&line).map_err(|_| ()))
            .and_then(|value| run(&context, value).map_err(|_| ()));
        println!(
            "FIXTURE:{}",
            match result {
                Ok(value) => json!({"ok":true,"value":value}),
                Err(_) => json!({"ok":false}),
            }
        );
        let _ = io::stdout().flush();
    }
}

#[cfg(target_os = "linux")]
fn shortcut_fixture() -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::{connection::Connection, protocol::xproto::*, rust_connection::RustConnection};
    let (conn, screen) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let first = conn.setup().min_keycode;
    let map = conn
        .get_keyboard_mapping(first, conn.setup().max_keycode - first + 1)?
        .reply()?;
    let key = first
        + map
            .keysyms
            .chunks(usize::from(map.keysyms_per_keycode))
            .position(|c| c.contains(&u32::from(b'j')))
            .ok_or("key")? as u8;
    conn.grab_key(
        false,
        root,
        ModMask::CONTROL | ModMask::M1,
        key,
        GrabMode::ASYNC,
        GrabMode::ASYNC,
    )?
    .check()?;
    conn.flush()?;
    println!("FIXTURE:{}", json!({"ok":true,"value":{"grabbed":true}}));
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let _ = line?;
    }
    Ok(())
}
