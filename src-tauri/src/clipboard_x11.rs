//! Bounded X11 selection reader. Unlike clipboard-rs's 50ms-per-conversion polling,
//! this drains notifications at 1ms and never reserves an untrusted INCR length.
use clipboard_rs::{common::RustImage, RustImageData};
use std::{
    io::Cursor,
    thread,
    time::{Duration, Instant},
};
use uclip_core::{AppError, ClipPayload, ErrorCode, MAX_IMAGE_PIXELS};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::RustConnection,
    COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE,
};
fn error() -> AppError {
    AppError::new(ErrorCode::Clipboard)
}
struct WindowGuard<'a>(&'a RustConnection, Window);
impl Drop for WindowGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.destroy_window(self.1);
        let _ = self.0.flush();
    }
}
pub struct Reader {
    conn: RustConnection,
    root: Window,
    clipboard: Atom,
    property: Atom,
    incr: Atom,
}
impl Reader {
    pub fn owner(&self) -> Result<Window, AppError> {
        self.conn
            .get_selection_owner(self.clipboard)
            .map_err(|_| error())?
            .reply()
            .map(|reply| reply.owner)
            .map_err(|_| error())
    }
    pub fn source_app(&self, owner: Window) -> Option<String> {
        crate::clipboard_source::application(&self.conn, self.root, owner)
    }
    pub fn new() -> Result<Self, AppError> {
        let (conn, screen) = RustConnection::connect(None).map_err(|_| error())?;
        let atom = |name: &[u8]| {
            conn.intern_atom(false, name)
                .map_err(|_| error())?
                .reply()
                .map(|r| r.atom)
                .map_err(|_| error())
        };
        let clipboard = atom(b"CLIPBOARD")?;
        let property = atom(b"_UCLIP_CAPTURE")?;
        let incr = atom(b"INCR")?;
        let root = conn.setup().roots[screen].root;
        Ok(Self {
            conn,
            root,
            clipboard,
            property,
            incr,
        })
    }
    pub fn formats(&self) -> Result<Vec<String>, AppError> {
        if self
            .conn
            .get_selection_owner(self.clipboard)
            .map_err(|_| error())?
            .reply()
            .map_err(|_| error())?
            .owner
            == NONE
        {
            return Ok(vec![]);
        }
        let bytes = self.buffer("TARGETS", 1024)?;
        if !bytes.len().is_multiple_of(4) {
            return Err(error());
        }
        let formats: Vec<String> = bytes
            .chunks_exact(4)
            .map(|b| {
                let atom = u32::from_ne_bytes(b.try_into().map_err(|_| error())?);
                String::from_utf8(
                    self.conn
                        .get_atom_name(atom)
                        .map_err(|_| error())?
                        .reply()
                        .map_err(|_| error())?
                        .name,
                )
                .map_err(|_| error())
            })
            .collect::<Result<_, AppError>>()?;
        if formats.iter().any(|f| f.len() > 256) {
            return Err(AppError::new(ErrorCode::LimitExceeded));
        }
        Ok(formats)
    }
    pub fn buffer(&self, format: &str, limit: usize) -> Result<Vec<u8>, AppError> {
        let atom = self
            .conn
            .intern_atom(false, format.as_bytes())
            .map_err(|_| error())?
            .reply()
            .map_err(|_| error())?
            .atom;
        let window = self.conn.generate_id().map_err(|_| error())?;
        self.conn
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                window,
                self.root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(|_| error())?
            .check()
            .map_err(|_| error())?;
        let _window = WindowGuard(&self.conn, window);
        self.conn
            .convert_selection(window, self.clipboard, atom, self.property, CURRENT_TIME)
            .map_err(|_| error())?;
        self.conn.flush().map_err(|_| error())?;
        let deadline = Instant::now() + Duration::from_millis(800);
        let mut incremental = false;
        let mut bytes = vec![];
        loop {
            if Instant::now() >= deadline {
                return Err(error());
            }
            let event = match self.conn.poll_for_event().map_err(|_| error())? {
                Some(e) => e,
                None => {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
            };
            let ready = match event {
                Event::SelectionNotify(e) if e.requestor == window => {
                    if e.property == NONE {
                        return Err(error());
                    }
                    true
                }
                Event::PropertyNotify(e)
                    if incremental
                        && e.window == window
                        && e.atom == self.property
                        && e.state == Property::NEW_VALUE =>
                {
                    true
                }
                _ => false,
            };
            if !ready {
                continue;
            }
            let remaining = limit.saturating_sub(bytes.len());
            let reply = self
                .conn
                .get_property(
                    true,
                    window,
                    self.property,
                    AtomEnum::ANY,
                    0,
                    remaining.div_ceil(4) as u32,
                )
                .map_err(|_| error())?
                .reply()
                .map_err(|_| error())?;
            if reply.type_ == self.incr {
                if incremental {
                    return Err(error());
                }
                let size = reply
                    .value32()
                    .and_then(|mut v| v.next())
                    .ok_or_else(error)? as usize;
                if size > limit {
                    return Err(AppError::new(ErrorCode::LimitExceeded));
                }
                incremental = true;
                self.conn.flush().map_err(|_| error())?;
                continue;
            }
            if reply.bytes_after > 0 || reply.value.len() > remaining {
                return Err(AppError::new(ErrorCode::LimitExceeded));
            }
            if reply.type_ != atom
                && !(format == "TARGETS" && reply.type_ == u32::from(AtomEnum::ATOM))
            {
                return Err(error());
            }
            if !incremental || reply.value.is_empty() {
                bytes.extend(reply.value);
                return Ok(bytes);
            }
            bytes.extend(reply.value);
            self.conn.flush().map_err(|_| error())?;
        }
    }
    pub fn payload(&self, formats: &[String]) -> Result<Option<ClipPayload>, AppError> {
        if let Some(format) = [
            "text/uri-list",
            "x-special/gnome-copied-files",
            "x-special/nautilus-clipboard",
        ]
        .iter()
        .find(|format| formats.iter().any(|f| f == **format))
        {
            let data = self.buffer(format, 8 * 1024 * 1024)?;
            let text =
                String::from_utf8(data).map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
            let files = text
                .lines()
                .skip(if *format == "text/uri-list" { 0 } else { 1 })
                .filter(|s| !s.is_empty() && !s.starts_with('#'))
                .map(String::from)
                .collect();
            return Ok(Some(ClipPayload::Files(super::clipboard::normalize_files(
                files,
            )?)));
        }
        if let Some(format) = ["image/png", "image/jpeg"]
            .iter()
            .find(|format| formats.iter().any(|f| f == **format))
        {
            let data = self.buffer(format, 40 * 1024 * 1024)?;
            let mut reader = image::ImageReader::new(Cursor::new(data))
                .with_guessed_format()
                .map_err(|_| error())?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let image = RustImageData::from_dynamic_image(reader.decode().map_err(|e| {
                AppError::new(if matches!(e, image::ImageError::Limits(_)) {
                    ErrorCode::LimitExceeded
                } else {
                    ErrorCode::InvalidInput
                })
            })?);
            let (width, height) = image.get_size();
            if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
                return Err(AppError::new(ErrorCode::LimitExceeded));
            }
            return Ok(Some(ClipPayload::Image {
                width,
                height,
                rgba: image.to_rgba8().map_err(|_| error())?.into_raw(),
            }));
        }
        if let Some(format) = [
            "UTF8_STRING",
            "text/plain;charset=utf-8",
            "text/plain;charset=UTF-8",
            "text/plain",
            "STRING",
        ]
        .iter()
        .find(|format| formats.iter().any(|f| f == **format))
        {
            let bytes = self.buffer(format, uclip_core::MAX_TEXT_BYTES)?;
            let text = if *format == "STRING" {
                bytes.into_iter().map(char::from).collect()
            } else {
                String::from_utf8(bytes).map_err(|_| AppError::new(ErrorCode::InvalidInput))?
            };
            return Ok(if text.is_empty() {
                None
            } else {
                Some(ClipPayload::Text(text))
            });
        }
        Ok(None)
    }
}
