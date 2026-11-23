//! Automatic paste is deliberately independent from clipboard write success.
#[cfg(target_os = "windows")]
mod windows;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use uclip_core::{AppError, ErrorCode};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InvocationTarget {
    #[cfg(target_os = "linux")]
    window: (u32, u32),
    #[cfg(target_os = "windows")]
    focus: windows::Focus,
}

pub struct PreparedPaste {
    #[cfg(target_os = "linux")]
    target: (u32, u32, std::time::Instant),
    #[cfg(target_os = "windows")]
    target: windows::Prepared,
}

pub struct PasteService {
    #[cfg(target_os = "linux")]
    inner: Option<Arc<Mutex<x11::Desktop>>>,
    #[cfg(target_os = "windows")]
    inner: Option<Arc<Mutex<windows::Desktop>>>,
    #[cfg(target_os = "windows")]
    app: Option<tauri::AppHandle>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl PasteService {
    pub fn start(app: Option<tauri::AppHandle>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "linux")]
        {
            if let Ok(desktop) = x11::Desktop::new() {
                let inner = Arc::new(Mutex::new(desktop));
                let worker = inner.clone();
                let stopped = stop.clone();
                let thread = std::thread::Builder::new()
                    .name("uclip-desktop-target".into())
                    .spawn(move || {
                        use tauri::Manager;
                        while !stopped.load(Ordering::SeqCst) {
                            let show = match worker.lock() {
                                Ok(mut d) => match d.observe() {
                                    Ok(show) => show,
                                    Err(_) => {
                                        d.healthy = false;
                                        break;
                                    }
                                },
                                Err(_) => break,
                            };
                            if show {
                                if let Some(window) =
                                    app.as_ref().and_then(|a| a.get_webview_window("main"))
                                {
                                    let _ = window
                                        .show()
                                        .and_then(|_| window.unminimize())
                                        .and_then(|_| window.set_focus());
                                }
                            }
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                    })
                    .ok();
                if thread.is_none() {
                    return Self {
                        inner: None,
                        stop,
                        thread: Mutex::new(None),
                    };
                }
                return Self {
                    inner: Some(inner),
                    stop,
                    thread: Mutex::new(thread),
                };
            }
        }
        #[cfg(target_os = "windows")]
        {
            let desktop = Arc::new(Mutex::new(windows::Desktop::default()));
            let worker = desktop.clone();
            let stopped = stop.clone();
            let thread = std::thread::Builder::new()
                .name("uclip-desktop-target".into())
                .spawn(move || {
                    while !stopped.load(Ordering::SeqCst) {
                        let Ok(mut desktop) = worker.lock() else {
                            break;
                        };
                        desktop.observe();
                        drop(desktop);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                })
                .ok();
            Self {
                inner: thread.as_ref().map(|_| desktop),
                app,
                stop,
                thread: Mutex::new(thread),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let _ = app;
        #[cfg(not(target_os = "windows"))]
        Self {
            #[cfg(target_os = "linux")]
            inner: None,
            stop,
            thread: Mutex::new(None),
        }
    }
    pub fn available(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            self.inner.is_some()
        }
        #[cfg(target_os = "linux")]
        {
            self.inner
                .as_ref()
                .and_then(|d| d.lock().ok())
                .is_some_and(|d| d.healthy)
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            false
        }
    }
    pub fn has_target(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            self.prepare().is_ok()
        }
        #[cfg(target_os = "linux")]
        {
            self.inner
                .as_ref()
                .and_then(|d| d.lock().ok())
                .is_some_and(|d| d.target.is_some())
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            false
        }
    }
    pub fn capture_target(&self) {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(inner) = &self.inner {
            if let Ok(mut desktop) = inner.lock() {
                #[cfg(target_os = "linux")]
                let _ = desktop.capture_target();
                #[cfg(target_os = "windows")]
                desktop.observe();
            }
        }
    }
    pub fn invocation_target(&self) -> Option<InvocationTarget> {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            let mut desktop = self.inner.as_ref()?.lock().ok()?;
            #[cfg(target_os = "linux")]
            {
                desktop.capture_target().ok()?;
                let (window, pid, _) = desktop.target?;
                Some(InvocationTarget {
                    window: (window, pid),
                })
            }
            #[cfg(target_os = "windows")]
            {
                Some(InvocationTarget {
                    focus: desktop.invocation_focus()?,
                })
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        None
    }

    pub fn prepare_invocation(
        &self,
        original: InvocationTarget,
        active: &AtomicBool,
    ) -> Result<PreparedPaste, AppError> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(700);
        loop {
            if !active.load(Ordering::Acquire) {
                return Err(AppError::new(ErrorCode::TargetLost));
            }
            let prepared = self.prepare()?;
            if !prepared.matches(original) {
                return Err(AppError::new(ErrorCode::TargetLost));
            }
            if self.modifiers_released()? {
                return Ok(prepared);
            }
            if std::time::Instant::now() >= deadline {
                return Err(AppError::new(ErrorCode::TargetLost));
            }
            // Wait outside the clipboard target lock and X11 server guard.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn modifiers_released(&self) -> Result<bool, AppError> {
        #[cfg(target_os = "linux")]
        {
            self.inner
                .as_ref()
                .ok_or_else(|| AppError::new(ErrorCode::Restricted))?
                .lock()
                .map_err(|_| AppError::new(ErrorCode::Unavailable))?
                .modifiers_released()
        }
        #[cfg(target_os = "windows")]
        {
            Ok(windows::modifiers_released())
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        Err(AppError::new(ErrorCode::Restricted))
    }

    pub fn shortcut_available(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.inner
                .as_ref()
                .and_then(|d| d.lock().ok())
                .is_some_and(|d| d.healthy && d.shortcut)
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
    pub fn prepare(&self) -> Result<PreparedPaste, AppError> {
        #[cfg(target_os = "windows")]
        {
            let target = self
                .inner
                .as_ref()
                .ok_or_else(|| AppError::new(ErrorCode::Restricted))?
                .lock()
                .map_err(|_| AppError::new(ErrorCode::Unavailable))?
                .prepare()?;
            Ok(PreparedPaste { target })
        }
        #[cfg(target_os = "linux")]
        {
            let desktop = self
                .inner
                .as_ref()
                .ok_or_else(|| AppError::new(ErrorCode::Restricted))?
                .lock()
                .map_err(|_| AppError::new(ErrorCode::Unavailable))?;
            desktop.validate()?;
            Ok(PreparedPaste {
                target: desktop
                    .target
                    .ok_or_else(|| AppError::new(ErrorCode::TargetLost))?,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            Err(AppError::new(ErrorCode::Restricted))
        }
    }
    pub fn paste(
        &self,
        prepared: PreparedPaste,
        #[cfg(target_os = "windows")] publication: uclip_runtime::clipboard::ClipboardWrite,
        #[cfg(target_os = "linux")] selection_owner: Option<u32>,
    ) -> Result<(), AppError> {
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let _ = prepared;
        #[cfg(target_os = "windows")]
        {
            self.inner
                .as_ref()
                .ok_or_else(|| AppError::new(ErrorCode::Restricted))?
                .lock()
                .map_err(|_| AppError::new(ErrorCode::Unavailable))?
                .paste(prepared.target, publication, self.app.as_ref())
        }
        #[cfg(target_os = "linux")]
        {
            self.inner
                .as_ref()
                .ok_or_else(|| AppError::new(ErrorCode::Restricted))?
                .lock()
                .map_err(|_| AppError::new(ErrorCode::Unavailable))?
                .paste(prepared.target, selection_owner)
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            Err(AppError::new(ErrorCode::Restricted))
        }
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut thread) = self.thread.lock() {
            if let Some(handle) = thread.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for PasteService {
    fn drop(&mut self) {
        self.stop();
    }
}

impl PreparedPaste {
    fn matches(&self, invocation: InvocationTarget) -> bool {
        #[cfg(target_os = "linux")]
        {
            (self.target.0, self.target.1) == invocation.window
        }
        #[cfg(target_os = "windows")]
        {
            self.target.matches(invocation.focus)
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = invocation;
            false
        }
    }
}

#[cfg(target_os = "linux")]
mod x11 {
    use super::*;
    use std::time::{Duration, Instant};
    use x11rb::{
        connection::Connection,
        protocol::{xproto::*, xtest},
        rust_connection::RustConnection,
        CURRENT_TIME, NONE,
    };
    fn fail() -> AppError {
        AppError::new(ErrorCode::TargetLost)
    }
    // Held only around final focus verification and key dispatch: never around
    // clipboard transfer, window activation, sleeps, storage or another client API.
    struct ServerGuard<'a>(&'a RustConnection);
    impl Drop for ServerGuard<'_> {
        fn drop(&mut self) {
            let _ = self.0.ungrab_server();
            let _ = self.0.flush();
        }
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum PasteShortcut {
        ControlV,
        ControlShiftV,
        ShiftInsert,
        CopyOnly,
    }

    fn paste_shortcut(wm_class: &[u8]) -> PasteShortcut {
        // WM_CLASS contains two NUL-terminated strings: instance and class.
        let class = wm_class.split(|&byte| byte == 0).nth(1).unwrap_or_default();
        match class.to_ascii_lowercase().as_slice() {
            b"gnome-terminal" | b"konsole" | b"kitty" | b"xfce4-terminal" | b"mate-terminal"
            | b"terminator" | b"tilix" => PasteShortcut::ControlShiftV,
            b"tabby" => PasteShortcut::ShiftInsert,
            b"xterm" | b"uxterm" | b"rxvt" | b"urxvt" => PasteShortcut::CopyOnly,
            _ => PasteShortcut::ControlV,
        }
    }

    struct PasteKeys {
        key: u8,
        modifiers: [Option<u8>; 2],
    }

    fn paste_keys(
        first: u8,
        per_key: u8,
        symbols: &[u32],
        shortcut: PasteShortcut,
    ) -> Result<PasteKeys, AppError> {
        if per_key == 0 || !symbols.len().is_multiple_of(usize::from(per_key)) {
            return Err(fail());
        }
        let find = |symbol| {
            symbols
                .chunks(usize::from(per_key))
                .position(|key| key[0] == symbol)
                .and_then(|i| u8::try_from(i).ok()?.checked_add(first))
                .ok_or_else(fail)
        };
        let control = || find(0xffe3);
        let shift = || find(0xffe1);
        let v = || find(u32::from(b'v'));
        let (key, modifiers) = match shortcut {
            PasteShortcut::ControlV => (v()?, [Some(control()?), None]),
            PasteShortcut::ControlShiftV => (v()?, [Some(control()?), Some(shift()?)]),
            PasteShortcut::ShiftInsert => (find(0xff63)?, [None, Some(shift()?)]),
            PasteShortcut::CopyOnly => return Err(AppError::new(ErrorCode::Restricted)),
        };
        Ok(PasteKeys { key, modifiers })
    }
    pub struct Desktop {
        conn: RustConnection,
        root: Window,
        active_atom: Atom,
        pid_atom: Atom,
        clipboard_atom: Atom,
        pub healthy: bool,
        pub shortcut: bool,
        pub target: Option<(Window, u32, Instant)>,
        previous: Window,
        last_external: Option<(Window, u32)>,
    }
    impl Desktop {
        pub fn new() -> Result<Self, AppError> {
            if std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland")
                || std::env::var_os("WAYLAND_DISPLAY").is_some()
            {
                return Err(AppError::new(ErrorCode::Restricted));
            }
            let (conn, screen) = RustConnection::connect(None).map_err(|_| fail())?;
            let root = conn.setup().roots[screen].root;
            let active_atom = conn
                .intern_atom(false, b"_NET_ACTIVE_WINDOW")
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?
                .atom;
            let pid_atom = conn
                .intern_atom(false, b"_NET_WM_PID")
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?
                .atom;
            let clipboard_atom = conn
                .intern_atom(false, b"CLIPBOARD")
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?
                .atom;
            xtest::get_version(&conn, 2, 2)
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?;
            conn.flush().map_err(|_| fail())?;
            Ok(Self {
                conn,
                root,
                active_atom,
                pid_atom,
                clipboard_atom,
                healthy: true,
                shortcut: false,
                target: None,
                previous: NONE,
                last_external: None,
            })
        }
        fn property(&self, window: Window, atom: Atom) -> Result<u32, AppError> {
            Ok(self
                .conn
                .get_property(false, window, atom, AtomEnum::ANY, 0, 1)
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?
                .value32()
                .and_then(|mut v| v.next())
                .unwrap_or(0))
        }
        fn active(&self) -> Result<Window, AppError> {
            self.property(self.root, self.active_atom)
        }
        pub fn observe(&mut self) -> Result<bool, AppError> {
            let active = self.active()?;
            let pid = if active != NONE {
                self.property(active, self.pid_atom).unwrap_or(0)
            } else {
                0
            };
            if active != self.previous {
                if pid == std::process::id() {
                    self.target = self
                        .last_external
                        .map(|(window, pid)| (window, pid, Instant::now()));
                } else {
                    self.target = None;
                    self.last_external = if pid != 0 { Some((active, pid)) } else { None };
                }
                self.previous = active;
            }
            while self.conn.poll_for_event().map_err(|_| fail())?.is_some() {}
            Ok(false)
        }

        pub fn capture_target(&mut self) -> Result<(), AppError> {
            self.target = None;
            self.last_external = None;
            let active = self.active()?;
            let pid = self.property(active, self.pid_atom)?;
            if pid != 0 && pid != std::process::id() {
                self.target = Some((active, pid, Instant::now()));
                self.last_external = Some((active, pid));
            }
            Ok(())
        }
        pub fn validate(&self) -> Result<(), AppError> {
            if !self.healthy {
                return Err(AppError::new(ErrorCode::Restricted));
            }
            let (window, pid, when) = self.target.ok_or_else(fail)?;
            let active = self.active()?;
            if when.elapsed() > Duration::from_secs(300)
                || active == NONE
                || self.property(active, self.pid_atom)? != std::process::id()
                || self.property(window, self.pid_atom)? != pid
                || self
                    .conn
                    .get_window_attributes(window)
                    .map_err(|_| fail())?
                    .reply()
                    .map_err(|_| fail())?
                    .map_state
                    != MapState::VIEWABLE
            {
                return Err(fail());
            }
            Ok(())
        }
        fn focused_window_in_target(&self, target: Window) -> Result<Option<Window>, AppError> {
            let mut focus = self
                .conn
                .get_input_focus()
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?
                .focus;
            let focused = focus;
            for _ in 0..16 {
                if focus == target {
                    return Ok(Some(focused));
                }
                if focus == NONE
                    || focus == self.root
                    || focus == u32::from(InputFocus::POINTER_ROOT)
                {
                    break;
                }
                focus = self
                    .conn
                    .query_tree(focus)
                    .map_err(|_| fail())?
                    .reply()
                    .map_err(|_| fail())?
                    .parent;
            }
            Ok((focus == target).then_some(focused))
        }
        fn shortcut_for(&self, target: Window) -> Result<PasteShortcut, AppError> {
            let class = self
                .conn
                .get_property(false, target, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 64)
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?;
            Ok(paste_shortcut(&class.value))
        }
        pub fn modifiers_released(&self) -> Result<bool, AppError> {
            let modifiers = u16::from(
                self.conn
                    .query_pointer(self.root)
                    .map_err(|_| fail())?
                    .reply()
                    .map_err(|_| fail())?
                    .mask,
            );
            Ok(modifiers
                & (u16::from(KeyButMask::SHIFT)
                    | u16::from(KeyButMask::CONTROL)
                    | u16::from(KeyButMask::MOD1)
                    | u16::from(KeyButMask::MOD3)
                    | u16::from(KeyButMask::MOD4)
                    | u16::from(KeyButMask::MOD5))
                == 0)
        }

        pub fn paste(
            &mut self,
            expected: (Window, u32, Instant),
            selection_owner: Option<Window>,
        ) -> Result<(), AppError> {
            self.validate()?;
            if self.target != Some(expected) || !self.modifiers_released()? {
                return Err(fail());
            }
            let (target, _, _) = expected;
            let shortcut = self.shortcut_for(target)?;
            if shortcut == PasteShortcut::CopyOnly {
                return Err(AppError::new(ErrorCode::Restricted));
            }
            let original = self.active()?;
            let event = ClientMessageEvent::new(
                32,
                target,
                self.active_atom,
                ClientMessageData::from([2, CURRENT_TIME, original, 0, 0]),
            );
            self.conn
                .send_event(
                    false,
                    self.root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    event,
                )
                .map_err(|_| fail())?;
            self.conn.flush().map_err(|_| fail())?;
            let deadline = Instant::now() + Duration::from_millis(400);
            loop {
                let current = self.active()?;
                // Some WMs publish the active window before moving input focus.
                // Wait outside the server guard so the WM can finish both requests.
                if current == target && self.focused_window_in_target(target)?.is_some() {
                    break;
                }
                if (current != original && current != target) || Instant::now() >= deadline {
                    return Err(fail());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            // Block other clients' focus-change requests only during this tiny dispatch.
            // A guard releases the server on every error/panic; a closed connection also releases it.
            self.conn
                .grab_server()
                .map_err(|_| fail())?
                .check()
                .map_err(|_| fail())?;
            let _server = ServerGuard(&self.conn);
            // Recheck after activation: the user may press a modifier while the WM
            // is restoring focus. Resolve the current map under the same guard so
            // xmodmap/layout changes cannot leave us injecting cached keycodes.
            if self
                .conn
                .query_pointer(self.root)
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?
                .mask
                .intersects(
                    KeyButMask::SHIFT
                        | KeyButMask::CONTROL
                        | KeyButMask::MOD1
                        | KeyButMask::MOD3
                        | KeyButMask::MOD4
                        | KeyButMask::MOD5,
                )
            {
                return Err(fail());
            }
            let first = self.conn.setup().min_keycode;
            let count = self.conn.setup().max_keycode - first + 1;
            let map = self
                .conn
                .get_keyboard_mapping(first, count)
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?;
            let keys = paste_keys(first, map.keysyms_per_keycode, &map.keysyms, shortcut)?;
            let modifiers = self
                .conn
                .get_modifier_mapping()
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?;
            let stride = usize::from(modifiers.keycodes_per_modifier());
            for (index, key) in [(2, keys.modifiers[0]), (0, keys.modifiers[1])] {
                if key.is_some_and(|key| {
                    !modifiers.keycodes[index * stride..(index + 1) * stride].contains(&key)
                }) {
                    return Err(fail());
                }
            }

            let focus = self.focused_window_in_target(target)?.ok_or_else(fail)?;
            // Never inject into an arbitrary active window; recheck immediately before dispatch.
            if self.active()? != target
                || self.property(target, self.pid_atom)? != expected.1
                || self.shortcut_for(target)? != shortcut
            {
                return Err(fail());
            }
            if let Some(owner) = selection_owner {
                if self
                    .conn
                    .get_selection_owner(self.clipboard_atom)
                    .map_err(|_| fail())?
                    .reply()
                    .map_err(|_| fail())?
                    .owner
                    != owner
                {
                    return Err(fail());
                }
            }
            self.dispatch(focus, shortcut, keys)?;
            self.conn.flush().map_err(|_| fail())?;
            self.target = None;
            Ok(())
        }

        fn fake_key(&self, event: u8, key: u8) -> Result<(), AppError> {
            xtest::fake_input(&self.conn, event, key, CURRENT_TIME, NONE, 0, 0, 0)
                .map_err(|_| fail())?
                .check()
                .map_err(|_| fail())
        }

        fn dispatch(
            &self,
            focus: Window,
            shortcut: PasteShortcut,
            keys: PasteKeys,
        ) -> Result<(), AppError> {
            let result = (|| {
                for key in keys.modifiers.into_iter().flatten() {
                    self.fake_key(KEY_PRESS_EVENT, key)?;
                }
                if shortcut == PasteShortcut::ControlShiftV {
                    // Send V to the focused client so the root's global shortcut
                    // cannot intercept it. Some clients also track modifier presses.
                    self.send_terminal_key(focus, keys.key)
                } else {
                    let pressed = self.fake_key(KEY_PRESS_EVENT, keys.key);
                    let released = self.fake_key(KEY_RELEASE_EVENT, keys.key);
                    pressed.and(released)
                }
            })();
            let mut released = Ok(());
            for key in keys.modifiers.into_iter().rev().flatten() {
                released = released.and(self.fake_key(KEY_RELEASE_EVENT, key));
            }
            result.and(released)
        }

        fn send_terminal_key(&self, focus: Window, key: u8) -> Result<(), AppError> {
            let event = KeyPressEvent {
                response_type: KEY_PRESS_EVENT,
                detail: key,
                sequence: 0,
                time: CURRENT_TIME,
                root: self.root,
                event: focus,
                child: NONE,
                root_x: 0,
                root_y: 0,
                event_x: 0,
                event_y: 0,
                state: KeyButMask::CONTROL | KeyButMask::SHIFT,
                same_screen: true,
            };
            for response_type in [KEY_PRESS_EVENT, KEY_RELEASE_EVENT] {
                let event = KeyPressEvent {
                    response_type,
                    ..event
                };
                self.conn
                    .send_event(false, focus, EventMask::NO_EVENT, event)
                    .map_err(|_| fail())?
                    .check()
                    .map_err(|_| fail())?;
            }
            Ok(())
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn paste_keys_follow_remapping_and_refuse_symbols_only_on_shifted_levels() {
            let old = [
                u32::from(b'v'),
                u32::from(b'V'),
                0xffe3,
                0,
                u32::from(b'x'),
                0,
            ];
            let keys = paste_keys(8, 2, &old, PasteShortcut::ControlV).unwrap();
            assert_eq!((keys.key, keys.modifiers), (8, [Some(9), None]));
            let remapped = [
                u32::from(b'x'),
                0,
                0xffe3,
                0,
                u32::from(b'v'),
                u32::from(b'V'),
            ];
            let keys = paste_keys(8, 2, &remapped, PasteShortcut::ControlV).unwrap();
            assert_eq!((keys.key, keys.modifiers), (10, [Some(9), None]));
            assert!(paste_keys(
                8,
                2,
                &[u32::from(b'x'), u32::from(b'v'), 0xffe3, 0],
                PasteShortcut::ControlV,
            )
            .is_err());
            assert!(paste_keys(8, 0, &[], PasteShortcut::ControlV).is_err());
        }

        #[test]
        fn terminal_shortcuts_use_the_window_class_and_the_live_keymap() {
            let symbols = [b'v'.into(), 0xffe3, 0xff63, 0xffe1];
            let gnome = paste_shortcut(b"gnome-terminal-server\0Gnome-terminal\0");
            let keys = paste_keys(8, 1, &symbols, gnome).unwrap();
            assert_eq!((keys.key, keys.modifiers), (8, [Some(9), Some(11)]));
            let tabby = paste_shortcut(b"tabby\0Tabby\0");
            let keys = paste_keys(8, 1, &symbols, tabby).unwrap();
            assert_eq!((keys.key, keys.modifiers), (10, [None, Some(11)]));
            assert!(paste_shortcut(b"terminal-in-title\0Firefox\0") == PasteShortcut::ControlV);
            assert!(paste_shortcut(b"xterm\0XTerm\0") == PasteShortcut::CopyOnly);
        }
    }
}
