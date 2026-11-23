use super::*;
use crate::clipboard::Captured;
use ::windows::{
    core::{w, Owned, PCWSTR, PWSTR},
    Win32::{
        Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
        System::{
            DataExchange::*,
            Memory::*,
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Shell::{DragQueryFileW, HDROP},
            WindowsAndMessaging::*,
        },
    },
};
use std::path::Path;

#[derive(Clone, Copy)]
pub struct ClipboardWrite {
    owner: isize,
}

impl ClipboardWrite {
    pub fn is_current(self) -> bool {
        unsafe { GetClipboardOwner() }.ok() == Some(HWND(self.owner as _))
    }
}

// Created, used and destroyed on the clipboard actor. No HWND crosses threads.
pub struct Context {
    window: HWND,
}
impl Context {
    pub fn new() -> Result<Self, AppError> {
        // SAFETY: STATIC is a system class; HWND_MESSAGE creates no visible UI.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("UClip clipboard owner"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|_| error())?;
        Ok(Self { window })
    }
    pub fn pump(&self) {
        // Clipboard ownership receives synchronous Windows messages even though
        // payloads are eager. Service only this actor's window, with a per-wake bound.
        for _ in 0..64 {
            let mut message = MSG::default();
            // SAFETY: message is writable; HWND belongs to this actor thread.
            if !unsafe { PeekMessageW(&mut message, Some(self.window), 0, 0, PM_REMOVE) }.as_bool()
            {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    pub fn read(&self, marker: &[u8]) -> Result<Option<Captured>, AppError> {
        let guard = self.open()?;
        let owner = clipboard_owner();
        let Some(payload) = self.read_payload(marker, guard)? else {
            return Ok(None);
        };
        // Resolve metadata after releasing the clipboard lock. The caller also checks its sequence.
        let source_app = owner.and_then(|owner| {
            let name = process_name(owner.1)?;
            (clipboard_owner() == Some(owner)).then_some(name)
        });
        Ok(Some(Captured {
            payload,
            source_app,
        }))
    }
    fn read_payload(
        &self,
        marker: &[u8],
        guard: OpenGuard,
    ) -> Result<Option<ClipPayload>, AppError> {
        let formats = formats()?;
        if crate::clipboard::sensitive(&formats, |name| buffer(register(name)?, 16)) {
            return Ok(None);
        }
        let internal = register(crate::clipboard::MARKER)?;
        if has(internal)
            && buffer(internal, 64).is_ok_and(|v| {
                v.strip_prefix(marker)
                    .is_some_and(|padding| padding.iter().all(|b| *b == 0))
            })
        {
            return Ok(None);
        }
        if has(15) {
            let handle = unsafe { GetClipboardData(15) }.map_err(|_| error())?;
            // Bound the source block too, before asking Shell to interpret it.
            check_length(unsafe { GlobalSize(HGLOBAL(handle.0)) }, 16 * 1024 * 1024)?;
            let drop = HDROP(handle.0);
            let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
            if count == 0 || count > 256 {
                return Err(limit());
            }
            let mut paths = Vec::with_capacity(count as usize);
            for i in 0..count {
                let length = unsafe { DragQueryFileW(drop, i, None) };
                check_length(length as usize, 32768)?;
                let mut path = vec![0u16; length as usize + 1];
                if unsafe { DragQueryFileW(drop, i, Some(&mut path)) } != length {
                    return Err(error());
                }
                paths.push(String::from_utf16(&path[..length as usize]).map_err(|_| invalid())?);
            }
            return Ok(Some(ClipPayload::Files(crate::clipboard::normalize_files(
                paths,
            )?)));
        }
        let png_format = register("PNG")?;
        if has(png_format) {
            let bytes = buffer(png_format, MAX_ENCODED_IMAGE)?;
            drop(guard);
            return decode_image(&bytes, Some(image::ImageFormat::Png)).map(Some);
        }
        for format in [17, 8] {
            // CF_DIBV5 / CF_DIB (also synthesized from CF_BITMAP)
            if has(format) {
                let bytes = buffer(format, MAX_ENCODED_IMAGE)?;
                drop(guard);
                return decode_dib(bytes).map(Some);
            }
        }
        if has(13) {
            return utf16_text(&buffer(13, MAX_TEXT_BYTES * 2 + 64)?);
        }
        Ok(None)
    }
    pub fn write(
        &self,
        payload: ClipPayload,
        marker_format: &str,
        marker: &[u8],
    ) -> Result<ClipboardWrite, AppError> {
        let mut representations = match payload {
            ClipPayload::Text(value) => {
                if value.contains('\0') {
                    return Err(invalid());
                }
                vec![(
                    13,
                    value
                        .encode_utf16()
                        .chain([0])
                        .flat_map(u16::to_le_bytes)
                        .collect(),
                )]
            }
            ClipPayload::Image {
                width,
                height,
                rgba,
            } => {
                let bitmap = dib(width, height, &rgba);
                vec![(register("PNG")?, png(width, height, rgba)?), (17, bitmap)]
            }
            ClipPayload::Files(paths) => {
                let mut bytes = vec![0u8; 20];
                bytes[0..4].copy_from_slice(&20u32.to_le_bytes());
                bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
                for path in paths {
                    bytes.extend(path.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
                }
                bytes.extend([0, 0]);
                vec![
                    (15, bytes),
                    (
                        register("Preferred DropEffect")?,
                        1u32.to_le_bytes().to_vec(),
                    ),
                ]
            }
        };
        representations.push((register(marker_format)?, marker.to_vec()));
        // Allocate every representation before clearing the original clipboard.
        let mut prepared = representations
            .into_iter()
            .map(|(format, bytes)| Memory::new(&bytes).map(|memory| (format, memory)))
            .collect::<Result<Vec<_>, _>>()?;
        let _guard = self.open()?;
        unsafe { EmptyClipboard() }.map_err(|_| error())?;
        for (format, memory) in &mut prepared {
            // SAFETY: memory is a live GMEM_MOVEABLE block. Ownership transfers only on success.
            if unsafe { SetClipboardData(*format, Some(HANDLE(memory.0 .0))) }.is_err() {
                // A partial publication is never reported as copied, and never retried.
                let _ = unsafe { EmptyClipboard() };
                return Err(error());
            }
            memory.0 = HGLOBAL::default();
        }
        // While this actor serializes writes, a replacement publication changes ownership.
        Ok(ClipboardWrite {
            owner: self.window.0 as isize,
        })
    }
    fn open(&self) -> Result<OpenGuard, AppError> {
        // No automatic retry of OS writes. A busy clipboard is a safe explicit failure.
        unsafe { OpenClipboard(Some(self.window)) }.map_err(|_| error())?;
        Ok(OpenGuard)
    }
}

fn clipboard_owner() -> Option<(HWND, u32)> {
    // SAFETY: Windows provides the handle; the optional PID output is writable.
    let window = unsafe { GetClipboardOwner() }.ok()?;
    let mut pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    (thread != 0 && pid != 0).then_some((window, pid))
}

fn process_name(pid: u32) -> Option<String> {
    // SAFETY: Use a non-inheritable, query-only handle and a bounded writable UTF-16 buffer.
    let process =
        unsafe { Owned::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?) };
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            *process,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut length,
        )
    }
    .ok()?;
    let path = String::from_utf16(path.get(..length as usize)?).ok()?;
    executable_name(&path)
}

fn executable_name(path: &str) -> Option<String> {
    // Store the executable name only. The path can contain private usernames and directories.
    let name = Path::new(path).file_stem()?.to_str()?.trim();
    (!name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control))
        .then(|| name.to_owned())
}

impl Drop for Context {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.window) };
    }
}
struct OpenGuard;
impl Drop for OpenGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}
struct Memory(HGLOBAL);
impl Memory {
    fn new(bytes: &[u8]) -> Result<Self, AppError> {
        let memory = Self(
            unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len()) }
                .map_err(|_| error())?,
        );
        let pointer = unsafe { GlobalLock(memory.0) };
        if pointer.is_null() {
            return Err(error());
        }
        // SAFETY: GlobalAlloc reserved bytes.len() bytes and the block is locked.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len());
        }
        let _ = unsafe { GlobalUnlock(memory.0) };
        Ok(memory)
    }
}
impl Drop for Memory {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            let _ = unsafe { GlobalFree(Some(self.0)) };
        }
    }
}
fn has(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}
fn register(name: &str) -> Result<u32, AppError> {
    let wide = name.encode_utf16().chain([0]).collect::<Vec<_>>();
    let id = unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) };
    if id == 0 {
        Err(error())
    } else {
        Ok(id)
    }
}
fn formats() -> Result<Vec<String>, AppError> {
    let mut names = Vec::new();
    let mut previous = 0;
    loop {
        let id = unsafe { EnumClipboardFormats(previous) };
        if id == 0 {
            break;
        }
        if names.len() == 256 {
            return Err(limit());
        }
        let mut name = [0u16; 256];
        let length = unsafe { GetClipboardFormatNameW(id, &mut name) };
        names.push(if length > 0 {
            String::from_utf16(&name[..length as usize]).map_err(|_| invalid())?
        } else {
            format!("standard:{id}")
        });
        previous = id;
    }
    Ok(names)
}
fn buffer(format: u32, maximum: usize) -> Result<Vec<u8>, AppError> {
    let handle = unsafe { GetClipboardData(format) }.map_err(|_| error())?;
    let memory = HGLOBAL(handle.0);
    let length = unsafe { GlobalSize(memory) };
    check_length(length, maximum)?;
    if length == 0 {
        return Err(error());
    }
    let pointer = unsafe { GlobalLock(memory) };
    if pointer.is_null() {
        return Err(error());
    }
    // SAFETY: clipboard remains open; GlobalSize bounds this locked block.
    let result = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) }.to_vec();
    let _ = unsafe { GlobalUnlock(memory) };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::executable_name;

    #[test]
    fn source_metadata_keeps_only_a_bounded_executable_name() {
        assert_eq!(
            executable_name(r"C:\Users\Private\Example App.exe").as_deref(),
            Some("Example App")
        );
        assert_eq!(executable_name(""), None);
        assert_eq!(executable_name("bad\tname.exe"), None);
        assert_eq!(executable_name(&format!("{}.exe", "x".repeat(257))), None);
    }
}
