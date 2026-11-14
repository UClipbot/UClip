use crate::clipboard_native::ClipboardWrite;
use ::windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
            VK_SHIFT, VK_V,
        },
        WindowsAndMessaging::{
            GetAncestor, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo,
            GetWindowThreadProcessId, IsIconic, IsWindowVisible, PostMessageW, SetForegroundWindow,
            ShowWindow, GA_ROOT, GUITHREADINFO, GUI_INMENUMODE, GUI_INMOVESIZE, GUI_POPUPMENUMODE,
            GUI_SYSTEMMENUMODE, SW_HIDE, WM_PASTE,
        },
    },
};
use std::{
    mem::size_of,
    thread,
    time::{Duration, Instant},
};
use uclip_core::{AppError, ErrorCode};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Window {
    handle: isize,
    thread: u32,
    pid: u32,
}

impl Window {
    fn from_handle(handle: HWND) -> Option<Self> {
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(handle, Some(&mut pid)) };
        (thread != 0 && pid != 0).then_some(Self {
            handle: handle.0 as isize,
            thread,
            pid,
        })
    }

    fn hwnd(self) -> HWND {
        HWND(self.handle as _)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Focus {
    window: Window,
    control: Window,
}

impl Focus {
    fn read(window: Window) -> Option<Self> {
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetGUIThreadInfo(window.thread, &mut info) }.ok()?;
        let menu_modes = GUI_INMENUMODE | GUI_INMOVESIZE | GUI_POPUPMENUMODE | GUI_SYSTEMMENUMODE;
        if info.hwndActive != window.hwnd()
            || (info.flags & menu_modes).0 != 0
            || unsafe { GetAncestor(info.hwndFocus, GA_ROOT) } != window.hwnd()
        {
            return None;
        }
        let control = Window::from_handle(info.hwndFocus)?;
        Some(Self { window, control })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Target {
    focus: Focus,
    since: Instant,
}

pub(super) struct Prepared {
    target: Target,
    original: Window,
}

#[derive(Default)]
pub(super) struct Desktop {
    last_external: Option<Focus>,
    target: Option<Target>,
}

impl Desktop {
    pub fn observe(&mut self) {
        let Some(window) = foreground() else {
            return;
        };
        if window.pid == std::process::id() {
            if let Some(focus) = self.last_external.take() {
                self.target = Some(Target {
                    focus,
                    since: Instant::now(),
                });
            }
        } else {
            self.target = None;
            self.last_external = Focus::read(window);
        }
    }

    pub fn prepare(&mut self) -> Result<Prepared, AppError> {
        self.observe();
        let original = foreground()
            .filter(|window| window.pid == std::process::id())
            .ok_or_else(target_lost)?;
        let target = self.target.ok_or_else(target_lost)?;
        if target.since.elapsed() > Duration::from_secs(300)
            || Window::from_handle(target.focus.window.hwnd()) != Some(target.focus.window)
            || Window::from_handle(target.focus.control.hwnd()) != Some(target.focus.control)
            || !unsafe { IsWindowVisible(target.focus.window.hwnd()).as_bool() }
            || unsafe { IsIconic(target.focus.window.hwnd()).as_bool() }
        {
            return Err(target_lost());
        }
        Ok(Prepared { target, original })
    }

    pub fn paste(
        &mut self,
        prepared: Prepared,
        publication: ClipboardWrite,
        app: Option<&tauri::AppHandle>,
    ) -> Result<(), AppError> {
        let Prepared { target, original } = prepared;
        if self.target != Some(target)
            || foreground() != Some(original)
            || !publication.is_current()
        {
            return Err(target_lost());
        }
        let window = target.focus.window;
        let deadline = Instant::now() + Duration::from_millis(400);
        let app = app.ok_or_else(target_lost)?;
        use tauri::Manager;
        let can_hide = app.state::<crate::desktop::DesktopState>().recoverable();
        let (activated, completion) = std::sync::mpsc::sync_channel(1);
        app.run_on_main_thread(move || {
            let valid = Instant::now() < deadline
                && foreground() == Some(original)
                && publication.is_current();
            if valid {
                // If Windows declines explicit activation, hiding the foreground window
                // lets it restore the previous one. Dispatch still checks exact focus.
                if !unsafe { SetForegroundWindow(window.hwnd()).as_bool() }
                    && can_hide
                    && foreground() == Some(original)
                {
                    let _ = unsafe { ShowWindow(original.hwnd(), SW_HIDE) };
                }
            }
            let _ = activated.send(valid);
        })
        .map_err(|_| target_lost())?;
        if !completion
            .recv_timeout(Duration::from_millis(400))
            .unwrap_or(false)
        {
            return Err(target_lost());
        }
        let deadline = Instant::now() + Duration::from_millis(400);
        loop {
            let current = foreground();
            if current == Some(window) && Focus::read(window) == Some(target.focus) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(target_lost());
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !publication.is_current()
            || foreground() != Some(window)
            || Focus::read(window) != Some(target.focus)
        {
            return Err(target_lost());
        }
        dispatch(target.focus, publication)?;
        self.target = None;
        self.last_external = None;
        Ok(())
    }
}

fn dispatch(focus: Focus, publication: ClipboardWrite) -> Result<(), AppError> {
    let control = focus.control;
    let mut class = [0u16; 256];
    let length = unsafe { GetClassNameW(control.hwnd(), &mut class) };
    if length > 0 {
        let class = String::from_utf16_lossy(&class[..length as usize]).to_ascii_lowercase();
        // Standard Edit and RichEdit controls accept a paste message addressed to the exact control.
        if class == "edit"
            || class.starts_with("richedit")
            || class.starts_with("windowsforms10.edit.")
        {
            if !publication.is_current()
                || foreground() != Some(focus.window)
                || Focus::read(focus.window) != Some(focus)
            {
                return Err(target_lost());
            }
            return unsafe { PostMessageW(Some(control.hwnd()), WM_PASTE, WPARAM(0), LPARAM(0)) }
                .map_err(|_| target_lost());
        }
    }
    let keys = [
        key(VK_CONTROL, false),
        key(VK_V, false),
        key(VK_V, true),
        key(VK_CONTROL, true),
    ];
    if [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN, VK_V]
        .into_iter()
        .any(|key| unsafe { GetAsyncKeyState(i32::from(key.0)) } < 0)
        || !publication.is_current()
        || foreground() != Some(focus.window)
        || Focus::read(focus.window) != Some(focus)
    {
        return Err(target_lost());
    }
    // Send one ordered batch. Never retry a possibly delivered paste.
    let sent = unsafe { SendInput(&keys, size_of::<INPUT>() as i32) };
    if sent != keys.len() as u32 {
        if sent != 0 {
            let release = if sent == 2 { &keys[2..] } else { &keys[3..] };
            let _ = unsafe { SendInput(release, size_of::<INPUT>() as i32) };
        }
        return Err(target_lost());
    }
    Ok(())
}

fn foreground() -> Option<Window> {
    Window::from_handle(unsafe { GetForegroundWindow() })
}

fn key(code: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: code,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                ..Default::default()
            },
        },
    }
}

fn target_lost() -> AppError {
    AppError::new(ErrorCode::TargetLost)
}
