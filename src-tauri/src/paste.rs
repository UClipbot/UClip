//! Automatic paste is deliberately independent from clipboard write success.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use uclip_core::{AppError, ErrorCode};

pub struct PasteService {
    #[cfg(target_os = "linux")]
    inner: Option<Arc<Mutex<x11::Desktop>>>,
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
        #[cfg(not(target_os = "linux"))]
        let _ = app;
        Self {
            #[cfg(target_os = "linux")]
            inner: None,
            stop,
            thread: Mutex::new(None),
        }
    }
    pub fn available(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.inner
                .as_ref()
                .and_then(|d| d.lock().ok())
                .is_some_and(|d| d.healthy)
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
    pub fn has_target(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.inner
                .as_ref()
                .and_then(|d| d.lock().ok())
                .is_some_and(|d| d.target.is_some())
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
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
    pub fn validate(&self) -> Result<(), AppError> {
        #[cfg(target_os = "linux")]
        {
            self.inner
                .as_ref()
                .ok_or_else(|| AppError::new(ErrorCode::Restricted))?
                .lock()
                .map_err(|_| AppError::new(ErrorCode::Unavailable))?
                .validate()
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(AppError::new(ErrorCode::Restricted))
        }
    }
    pub fn paste(&self) -> Result<(), AppError> {
        #[cfg(target_os = "linux")]
        {
            self.inner
                .as_ref()
                .ok_or_else(|| AppError::new(ErrorCode::Restricted))?
                .lock()
                .map_err(|_| AppError::new(ErrorCode::Unavailable))?
                .paste()
        }
        #[cfg(not(target_os = "linux"))]
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

#[cfg(target_os = "linux")]
mod x11 {
    use super::*;
    use std::time::{Duration, Instant};
    use x11rb::{
        connection::Connection,
        protocol::{xproto::*, xtest, Event},
        rust_connection::RustConnection,
        CURRENT_TIME, NONE,
    };
    fn fail() -> AppError {
        AppError::new(ErrorCode::TargetLost)
    }
    // Held only around final focus verification and four key events: never around
    // clipboard transfer, window activation, sleeps, storage or another client API.
    struct ServerGuard<'a>(&'a RustConnection);
    impl Drop for ServerGuard<'_> {
        fn drop(&mut self) {
            let _ = self.0.ungrab_server();
            let _ = self.0.flush();
        }
    }
    pub struct Desktop {
        conn: RustConnection,
        root: Window,
        active_atom: Atom,
        pid_atom: Atom,
        pub healthy: bool,
        pub shortcut: bool,
        pub target: Option<(Window, u32, Instant)>,
        previous: Window,
        last_external: Option<(Window, u32)>,
        v_key: Keycode,
        ctrl_key: Keycode,
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
            xtest::get_version(&conn, 2, 2)
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?;
            let first = conn.setup().min_keycode;
            let count = conn.setup().max_keycode - first + 1;
            let map = conn
                .get_keyboard_mapping(first, count)
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?;
            let find = |symbol: u32| {
                map.keysyms
                    .chunks(usize::from(map.keysyms_per_keycode))
                    .position(|chunk| chunk.contains(&symbol))
                    .map(|i| first + i as u8)
                    .ok_or_else(fail)
            };
            let v_key = find(u32::from(b'v'))?;
            let ctrl_key = find(0xffe3)?;
            let mut shortcut = true;
            for extra in [
                ModMask::default(),
                ModMask::LOCK,
                ModMask::M2,
                ModMask::LOCK | ModMask::M2,
            ] {
                if conn
                    .grab_key(
                        false,
                        root,
                        ModMask::CONTROL | ModMask::SHIFT | extra,
                        v_key,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                    )
                    .map_err(|_| fail())?
                    .check()
                    .is_err()
                {
                    shortcut = false;
                }
            }
            if !shortcut {
                for extra in [
                    ModMask::default(),
                    ModMask::LOCK,
                    ModMask::M2,
                    ModMask::LOCK | ModMask::M2,
                ] {
                    let _ = conn.ungrab_key(v_key, root, ModMask::CONTROL | ModMask::SHIFT | extra);
                }
            }
            conn.flush().map_err(|_| fail())?;
            Ok(Self {
                conn,
                root,
                active_atom,
                pid_atom,
                healthy: true,
                shortcut,
                target: None,
                previous: NONE,
                last_external: None,
                v_key,
                ctrl_key,
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
                    if let Some((window, pid)) = self.last_external {
                        self.target = Some((window, pid, Instant::now()));
                    }
                } else if pid != 0 {
                    self.last_external = Some((active, pid));
                }
                self.previous = active;
            }
            let mut show = false;
            while let Some(event) = self.conn.poll_for_event().map_err(|_| fail())? {
                if matches!(event,Event::KeyPress(ref e) if e.detail==self.v_key) {
                    if pid != 0 && pid != std::process::id() {
                        self.target = Some((active, pid, Instant::now()));
                    }
                    show = true;
                }
            }
            Ok(show)
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
        pub fn paste(&mut self) -> Result<(), AppError> {
            self.validate()?;
            let (target, _, _) = self.target.ok_or_else(fail)?;
            let original = self.active()?;
            let modifiers = u16::from(
                self.conn
                    .query_pointer(self.root)
                    .map_err(|_| fail())?
                    .reply()
                    .map_err(|_| fail())?
                    .mask,
            );
            if modifiers
                & (u16::from(KeyButMask::SHIFT)
                    | u16::from(KeyButMask::CONTROL)
                    | u16::from(KeyButMask::MOD1)
                    | u16::from(KeyButMask::MOD4))
                != 0
            {
                return Err(fail());
            }
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
                if current == target {
                    break;
                }
                if current != original || Instant::now() >= deadline {
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
            let mut focus = self
                .conn
                .get_input_focus()
                .map_err(|_| fail())?
                .reply()
                .map_err(|_| fail())?
                .focus;
            let mut in_target = focus == target;
            for _ in 0..16 {
                if in_target
                    || focus == NONE
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
                in_target = focus == target;
            }
            if !in_target {
                return Err(fail());
            }
            // Never inject into an arbitrary active window; recheck immediately before dispatch.
            if self.active()? != target {
                return Err(fail());
            }
            for (event, key) in [
                (KEY_PRESS_EVENT, self.ctrl_key),
                (KEY_PRESS_EVENT, self.v_key),
                (KEY_RELEASE_EVENT, self.v_key),
                (KEY_RELEASE_EVENT, self.ctrl_key),
            ] {
                if xtest::fake_input(&self.conn, event, key, CURRENT_TIME, NONE, 0, 0, 0)
                    .map_err(|_| fail())
                    .and_then(|cookie| cookie.check().map_err(|_| fail()))
                    .is_err()
                {
                    let _ = xtest::fake_input(
                        &self.conn,
                        KEY_RELEASE_EVENT,
                        self.v_key,
                        CURRENT_TIME,
                        NONE,
                        0,
                        0,
                        0,
                    );
                    let _ = xtest::fake_input(
                        &self.conn,
                        KEY_RELEASE_EVENT,
                        self.ctrl_key,
                        CURRENT_TIME,
                        NONE,
                        0,
                        0,
                        0,
                    );
                    let _ = self.conn.flush();
                    return Err(fail());
                }
            }
            self.conn.flush().map_err(|_| fail())?;
            self.target = None;
            Ok(())
        }
    }
}
