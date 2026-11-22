//! Explicit user-triggered X11 activation. GTK present() alone may be refused as stale user input.
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use uclip_core::{AppError, ErrorCode};
use x11rb::{
    connection::Connection, protocol::xproto::*, rust_connection::RustConnection, CURRENT_TIME,
    NONE,
};

pub struct Activation {
    connection: RustConnection,
    root: Window,
    active_atom: Atom,
    pid_atom: Atom,
    previous: Window,
    previous_pid: u32,
}

pub fn window_id(window: &tauri::WebviewWindow) -> Result<Window, AppError> {
    let handle = window.window_handle().map_err(|_| failure())?;
    match handle.as_raw() {
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).map_err(|_| failure()),
        RawWindowHandle::Xcb(h) => Ok(h.window.get()),
        _ => Err(failure()),
    }
}

pub fn is_active_window(window: Window) -> bool {
    Activation::prepare().is_ok_and(|activation| activation.previous == window)
}

fn failure() -> AppError {
    AppError::new(ErrorCode::TargetLost)
}
fn property(connection: &RustConnection, window: Window, atom: Atom) -> Result<u32, AppError> {
    if window == NONE {
        return Ok(0);
    }
    Ok(connection
        .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)
        .map_err(|_| failure())?
        .reply()
        .map_err(|_| failure())?
        .value32()
        .and_then(|mut p| p.next())
        .unwrap_or(0))
}
impl Activation {
    pub fn prepare() -> Result<Self, AppError> {
        let (connection, screen) = RustConnection::connect(None).map_err(|_| failure())?;
        let root = connection.setup().roots[screen].root;
        let active_atom = connection
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")
            .map_err(|_| failure())?
            .reply()
            .map_err(|_| failure())?
            .atom;
        let pid_atom = connection
            .intern_atom(false, b"_NET_WM_PID")
            .map_err(|_| failure())?
            .reply()
            .map_err(|_| failure())?
            .atom;
        let mut activation = Self {
            connection,
            root,
            active_atom,
            pid_atom,
            previous: NONE,
            previous_pid: 0,
        };
        activation.previous = activation.active()?;
        activation.previous_pid = property(&activation.connection, activation.previous, pid_atom)?;
        Ok(activation)
    }
    fn active(&self) -> Result<Window, AppError> {
        Ok(self
            .connection
            .get_property(false, self.root, self.active_atom, AtomEnum::WINDOW, 0, 1)
            .map_err(|_| failure())?
            .reply()
            .map_err(|_| failure())?
            .value32()
            .and_then(|mut p| p.next())
            .unwrap_or(NONE))
    }
    pub fn request(self, window: &tauri::WebviewWindow) -> Result<(), AppError> {
        let main = window_id(window)?;
        if property(&self.connection, main, self.pid_atom)? != std::process::id() {
            return Err(failure());
        }
        let current = self.active()?;
        // A later foreground transition is not permission to steal focus back.
        if current != main
            && (current != self.previous
                || property(&self.connection, current, self.pid_atom)? != self.previous_pid)
        {
            return Err(failure());
        }
        let event = ClientMessageEvent::new(
            32,
            main,
            self.active_atom,
            ClientMessageData::from([2, CURRENT_TIME, self.previous, 0, 0]),
        );
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .map_err(|_| failure())?
            .check()
            .map_err(|_| failure())?;
        self.connection.flush().map_err(|_| failure())
    }
}
