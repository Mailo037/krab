//! Windows back-end: plain GDI. `BitBlt` from the screen DC needs no setup,
//! no COM and no GPU device, which is what keeps cold starts in the low
//! milliseconds.

use crate::capture::{Shot, WinInfo};
use crate::geom::RectI;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
    GetDC, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO, MONITORINFOEXW, SRCCOPY,
};
use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, EnumWindows, GetClassNameW, GetCursorPos, GetMessagePos,
    GetWindowLongW, GetWindowRect, GetWindowTextW, IsIconic, IsWindowVisible, ASFW_ANY,
    GWL_EXSTYLE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

const MONITORINFOF_PRIMARY: u32 = 1;

/// Must run before anything queries monitor geometry.
pub fn init_process() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Reconnect stdout/stderr to the launching terminal (GUI-subsystem binaries
/// start detached).
pub fn attach_console() {
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Let the already-running instance take focus when we hand a request to it.
pub fn allow_foreground_handoff() {
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

/// Screen position of the pointer for the input message being handled.
///
/// Windows synthesizes mouse-move messages lazily, so a button message can
/// overtake the move that preceded it. Asking for the position that belongs
/// to the button message itself keeps fast flick-and-release drags exact.
pub fn message_cursor_pos() -> Option<(i32, i32)> {
    let packed = unsafe { GetMessagePos() };
    Some((
        (packed & 0xFFFF) as i16 as i32,
        (packed >> 16) as i16 as i32,
    ))
}

/// Screen DC + memory DC + DIB section, reusable across grabs.
struct Blitter {
    screen: HDC,
    mem: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    bits: *const u32,
    w: i32,
    h: i32,
}

impl Blitter {
    fn new(w: i32, h: i32) -> Result<Self, String> {
        if w <= 0 || h <= 0 {
            return Err("empty capture area".into());
        }
        unsafe {
            let screen = GetDC(null_mut());
            if screen.is_null() {
                return Err("GetDC failed".into());
            }
            let mem = CreateCompatibleDC(screen);
            let mut bmi: BITMAPINFO = zeroed();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            let mut bits: *mut c_void = null_mut();
            let dib = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if dib.is_null() || bits.is_null() {
                DeleteDC(mem);
                ReleaseDC(null_mut(), screen);
                return Err("CreateDIBSection failed".into());
            }
            let old = SelectObject(mem, dib);
            Ok(Self {
                screen,
                mem,
                dib,
                old,
                bits: bits as *const u32,
                w,
                h,
            })
        }
    }

    /// Copy the screen area at (`x`, `y`) into `out` as `0xFFRRGGBB`.
    fn grab(&mut self, x: i32, y: i32, out: &mut Vec<u32>) -> Result<(), String> {
        let n = (self.w * self.h) as usize;
        unsafe {
            if BitBlt(
                self.mem,
                0,
                0,
                self.w,
                self.h,
                self.screen,
                x,
                y,
                SRCCOPY | CAPTUREBLT,
            ) == 0
            {
                return Err("BitBlt failed".into());
            }
            let src = std::slice::from_raw_parts(self.bits, n);
            out.clear();
            out.reserve_exact(n);
            // GDI leaves the alpha byte undefined; force it opaque.
            out.extend(src.iter().map(|p| p | 0xFF00_0000));
        }
        Ok(())
    }
}

impl Drop for Blitter {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem, self.old);
            DeleteObject(self.dib);
            DeleteDC(self.mem);
            ReleaseDC(null_mut(), self.screen);
        }
    }
}

struct MonitorDesc {
    rect: RectI,
    primary: bool,
    scale: f32,
    name: String,
}

unsafe extern "system" fn monitor_cb(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data as *mut Vec<MonitorDesc>);
    let mut info: MONITORINFOEXW = zeroed();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) != 0 {
        let r = info.monitorInfo.rcMonitor;
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let len = info
            .szDevice
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(info.szDevice.len());
        list.push(MonitorDesc {
            rect: RectI::new(r.left, r.top, r.right - r.left, r.bottom - r.top),
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            scale: dx as f32 / 96.0,
            name: String::from_utf16_lossy(&info.szDevice[..len]),
        });
    }
    1
}

fn monitors() -> Vec<MonitorDesc> {
    let mut list: Vec<MonitorDesc> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(monitor_cb),
            &mut list as *mut _ as LPARAM,
        );
    }
    list
}

pub fn capture_monitors() -> Result<Vec<Shot>, String> {
    let mut shots = Vec::new();
    for m in monitors() {
        let mut px = Vec::new();
        Blitter::new(m.rect.w, m.rect.h)?.grab(m.rect.x, m.rect.y, &mut px)?;
        shots.push(Shot {
            native: m.rect,
            w: m.rect.w as u32,
            h: m.rect.h as u32,
            px,
            scale: m.scale,
            primary: m.primary,
            name: m.name,
        });
    }
    Ok(shots)
}

unsafe extern "system" fn window_cb(hwnd: HWND, data: LPARAM) -> BOOL {
    let list = &mut *(data as *mut Vec<WinInfo>);
    if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
        return 1;
    }
    // UWP shells and windows on other virtual desktops are "visible" but cloaked.
    let mut cloaked: u32 = 0;
    DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED as u32,
        &mut cloaked as *mut u32 as *mut c_void,
        4,
    );
    if cloaked != 0 {
        return 1;
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TRANSPARENT != 0 {
        return 1; // click-through overlays
    }
    let mut buf = [0u16; 256];
    let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
    let title = String::from_utf16_lossy(&buf[..n]);
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
    let class = String::from_utf16_lossy(&buf[..n]);
    let is_taskbar = class == "Shell_TrayWnd" || class == "Shell_SecondaryTrayWnd";
    if matches!(class.as_str(), "Progman" | "WorkerW") {
        return 1; // the desktop itself; the monitor fallback covers it
    }
    if title.is_empty() && !is_taskbar && ex & WS_EX_TOOLWINDOW != 0 {
        return 1; // invisible helper windows
    }
    // Prefer the visible frame; GetWindowRect includes the invisible resize border.
    let mut r: RECT = zeroed();
    let hr = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS as u32,
        &mut r as *mut RECT as *mut c_void,
        size_of::<RECT>() as u32,
    );
    if hr != 0 && GetWindowRect(hwnd, &mut r) == 0 {
        return 1;
    }
    let rect = RectI::new(r.left, r.top, r.right - r.left, r.bottom - r.top);
    if rect.w > 8 && rect.h > 8 {
        list.push(WinInfo { rect, title });
    }
    1
}

/// Visible top-level windows, top-most first (EnumWindows walks the z-order).
pub fn list_windows() -> Vec<WinInfo> {
    let mut list: Vec<WinInfo> = Vec::new();
    unsafe {
        EnumWindows(Some(window_cb), &mut list as *mut _ as LPARAM);
    }
    list
}

pub struct Grabber {
    blitter: Blitter,
    x: i32,
    y: i32,
}

impl Grabber {
    pub fn new(_monitor: usize, monitor_native: RectI, local: RectI) -> Result<Self, String> {
        Ok(Self {
            blitter: Blitter::new(local.w, local.h)?,
            x: monitor_native.x + local.x,
            y: monitor_native.y + local.y,
        })
    }

    pub fn grab(&mut self, out: &mut Vec<u32>) -> Result<(), String> {
        self.blitter.grab(self.x, self.y, out)
    }
}

/// Spawn helpers without flashing a console window.
pub fn hide_console(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

// ---- Windows' own Print Screen snipping ---------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const KEYBOARD_KEY: &str = "Control Panel\\Keyboard";
const SNIP_VALUE: &str = "PrintScreenKeyForSnippingEnabled";

/// Whether Windows opens its own Snipping Tool on Print Screen (and so
/// competes with a Krab shortcut on that key). Windows 11 does this by default
/// when the setting was never touched.
pub fn print_screen_opens_snipping_tool() -> bool {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    let (key, name) = (wide(KEYBOARD_KEY), wide(SNIP_VALUE));
    let mut value: u32 = 0;
    let mut len = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut value as *mut u32 as *mut c_void,
            &mut len,
        )
    };
    if status == 0 {
        return value != 0;
    }
    // Never set: on by default from Windows 11 (build 22000) on.
    let (key, name) = (
        wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
        wide("CurrentBuildNumber"),
    );
    let mut buf = [0u16; 16];
    let mut len = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut len,
        )
    };
    if status != 0 {
        return false;
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
        .parse::<u32>()
        .is_ok_and(|build| build >= 22000)
}

/// Turn off "Use the Print Screen key to open screen capture" (the same
/// switch as in Settings > Accessibility > Keyboard).
pub fn disable_snipping_tool_on_print_screen() -> std::io::Result<()> {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };
    let (key, name) = (wide(KEYBOARD_KEY), wide(SNIP_VALUE));
    let value: u32 = 0;
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            REG_DWORD,
            &value as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        )
    };
    if status != 0 {
        return Err(std::io::Error::from_raw_os_error(status as i32));
    }
    // Tell running programs (Explorer) that a setting changed.
    let area = wide("Keyboard");
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            area.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            1000,
            null_mut(),
        );
    }
    Ok(())
}
