use uclip_core::{AppError, ErrorCode};

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn error() -> AppError {
    AppError::new(ErrorCode::Clipboard)
}

#[cfg(target_os = "linux")]
pub struct Watch {
    conn: x11rb::rust_connection::RustConnection,
    epoch: u64,
}
#[cfg(target_os = "linux")]
impl Watch {
    pub fn start() -> Result<Self, AppError> {
        use x11rb::{
            connection::Connection,
            protocol::{xfixes, xproto::ConnectionExt},
        };
        let (conn, screen) =
            x11rb::rust_connection::RustConnection::connect(None).map_err(|_| error())?;
        xfixes::query_version(&conn, 5, 0)
            .map_err(|_| error())?
            .reply()
            .map_err(|_| error())?;
        let clipboard = conn
            .intern_atom(false, b"CLIPBOARD")
            .map_err(|_| error())?
            .reply()
            .map_err(|_| error())?
            .atom;
        xfixes::select_selection_input(
            &conn,
            conn.setup().roots[screen].root,
            clipboard,
            xfixes::SelectionEventMask::SET_SELECTION_OWNER
                | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE
                | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY,
        )
        .map_err(|_| error())?
        .check()
        .map_err(|_| error())?;
        Ok(Self { conn, epoch: 0 })
    }
    pub fn generation(&mut self) -> Result<u64, AppError> {
        use x11rb::{
            connection::Connection,
            protocol::{xproto::ConnectionExt, Event},
        };
        // Round-trip before draining: resume excludes every notification preceding acknowledgement.
        self.conn
            .get_input_focus()
            .map_err(|_| error())?
            .reply()
            .map_err(|_| error())?;
        while let Some(event) = self.conn.poll_for_event().map_err(|_| error())? {
            if matches!(event, Event::XfixesSelectionNotify(_)) {
                self.epoch = self.epoch.wrapping_add(1);
            }
        }
        Ok(self.epoch)
    }
    pub fn stop(&mut self) -> Result<(), AppError> {
        Ok(())
    }
}

// Poll native sequence metadata on the clipboard actor, so pause/resume observes
// a synchronous boundary instead of a delayed watcher callback from before resume.
#[cfg(target_os = "windows")]
pub struct Watch;
#[cfg(target_os = "windows")]
impl Watch {
    pub fn start() -> Result<Self, AppError> {
        let mut watch = Self;
        watch.generation()?;
        Ok(watch)
    }
    pub fn generation(&mut self) -> Result<u64, AppError> {
        // SAFETY: GetClipboardSequenceNumber has no pointer arguments and may run on this worker.
        let sequence =
            unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        if sequence == 0 {
            return Err(AppError::new(ErrorCode::Restricted));
        }
        Ok(u64::from(sequence))
    }
    pub fn stop(&mut self) -> Result<(), AppError> {
        Ok(())
    }
}
#[cfg(target_os = "macos")]
pub struct Watch;
#[cfg(target_os = "macos")]
impl Watch {
    pub fn start() -> Result<Self, AppError> {
        let mut watch = Self;
        watch.generation()?;
        Ok(watch)
    }
    pub fn generation(&mut self) -> Result<u64, AppError> {
        objc2::rc::autoreleasepool(|_| {
            u64::try_from(objc2_app_kit::NSPasteboard::generalPasteboard().changeCount())
                .map_err(|_| error())
        })
    }
    pub fn stop(&mut self) -> Result<(), AppError> {
        Ok(())
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub struct Watch;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
impl Watch {
    pub fn start() -> Result<Self, AppError> {
        Err(AppError::new(ErrorCode::Restricted))
    }
    pub fn generation(&mut self) -> Result<u64, AppError> {
        Err(AppError::new(ErrorCode::Restricted))
    }
    pub fn stop(&mut self) -> Result<(), AppError> {
        Ok(())
    }
}
