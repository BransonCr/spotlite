use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use windows_sys::Win32::System::Console::{
    SetConsoleCtrlHandler, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
};

/// Windows Terminal's own font (Cascadia Mono) has no Nerd Font glyphs: plain ones unless
/// config.toml says `nerd_font = true`.
pub const NERD_DEFAULT: bool = false;

static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();

unsafe extern "system" fn on_console_event(event: u32) -> i32 {
    match event {
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            if let Some(stop) = STOP.get() {
                stop.store(true, Ordering::Relaxed);
            }
            // Windows ends the process when this returns: give the main loop time to save.
            std::thread::sleep(Duration::from_millis(1500));
            1
        }
        _ => 0,
    }
}

/// Sets `stop` when the console window closes, or at log off or shut down, so the main loop
/// ends cleanly and saves the state.
pub fn on_close(stop: Arc<AtomicBool>) {
    if STOP.set(stop).is_ok() {
        unsafe {
            SetConsoleCtrlHandler(Some(on_console_event), 1);
        }
    }
}

/// Windows tells of a closed console with an event (see `on_close`), not by a new parent.
pub fn parent_id() -> Option<u32> {
    None
}

/// The file is in the user's own profile, which other users cannot read.
pub fn private_file(_options: &mut std::fs::OpenOptions) {}

/// Makes `cmd` run on its own, with no console window, so it keeps running after spotlite and
/// its terminal close.
pub fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
}

/// A hidden window, owned by this thread: Windows ties the media controls (media keys, the
/// media flyout) to a window. Call `pump_messages` often on the same thread.
pub fn hidden_window() -> Option<*mut std::ffi::c_void> {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WS_OVERLAPPEDWINDOW};
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let title: Vec<u16> = "spotlite\0".encode_utf16().collect();
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    (!hwnd.is_null()).then_some(hwnd)
}

/// Handles the window messages that wait for this thread.
pub fn pump_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Puts `text` on the clipboard.
pub fn copy_text(text: &str) {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return;
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        if !mem.is_null() {
            let dst = GlobalLock(mem) as *mut u16;
            if dst.is_null() {
                GlobalFree(mem);
            } else {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
                GlobalUnlock(mem);
                // The clipboard owns the memory once this works.
                if SetClipboardData(CF_UNICODETEXT, mem).is_null() {
                    GlobalFree(mem);
                }
            }
        }
        CloseClipboard();
    }
}

/// A sans-serif font file for the share card: Inter if it is installed, else Segoe UI.
pub fn font_file(bold: bool) -> Result<PathBuf> {
    let names: &[&str] = if bold {
        &["Inter-Bold.ttf", "Inter-Bold.otf", "segoeuib.ttf", "arialbd.ttf"]
    } else {
        &["Inter-Regular.ttf", "Inter-Regular.otf", "segoeui.ttf", "arial.ttf"]
    };
    let mut dirs = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
    }
    dirs.push(PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts"));
    names
        .iter()
        .flat_map(|name| dirs.iter().map(move |d| d.join(name)))
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("no Inter or Segoe UI font found"))
}
