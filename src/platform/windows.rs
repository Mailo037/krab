//! Windows back-end: plain GDI. `BitBlt` from the screen DC needs no setup,
//! no COM and no GPU device, which is what keeps cold starts in the low
//! milliseconds.

use crate::capture::{Shot, WinInfo, WindowImage};
use crate::geom::RectI;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_CLOAKED,
    DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
    GdiFlush, GetDC, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO, MONITORINFOEXW,
    SRCCOPY,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, EnumWindows, GetClassNameW, GetCursorPos, GetMessagePos,
    GetWindowLongW, GetWindowRect, GetWindowTextW, IsHungAppWindow, IsIconic, IsWindow,
    IsWindowVisible, ASFW_ANY, GWL_EXSTYLE, PW_RENDERFULLCONTENT, WM_NCACTIVATE, WM_NCDESTROY,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, ICONINFO,
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

/// Whether Windows shows animations ("Animation effects" in the
/// accessibility settings); Krab's own fades follow it.
pub fn animations_enabled() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION,
    };
    let mut on: i32 = 1;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            &mut on as *mut i32 as *mut core::ffi::c_void,
            0,
        )
    };
    ok == 0 || on != 0
}

/// Let the already-running instance take focus when we hand a request to it.
pub fn allow_foreground_handoff() {
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

/// Keep `window` off the screen while Windows treats it as shown (DWM
/// cloaking). winit's borderless windows keep the caption style and only
/// drop the frame once they are shown, so for a moment Windows draws a white
/// title bar and an unpainted client area; showing a window cloaked, painting
/// it and then uncloaking it skips all of that.
pub fn cloak(window: &winit::window::Window, on: bool) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let value: BOOL = on.into();
    unsafe {
        DwmSetWindowAttribute(
            h.hwnd.get() as HWND,
            DWMWA_CLOAK as u32,
            &value as *const BOOL as *const c_void,
            size_of::<BOOL>() as u32,
        );
    }
}

/// Stop Windows from painting a classic title bar into a borderless window.
///
/// When a window gains or loses activation, `DefWindowProc` handles
/// `WM_NCACTIVATE` by drawing an old-style caption ("Krab" on a light-blue
/// gradient) straight into the window's pixels, even though our windows have
/// no caption style. It stays there until the next frame is presented, which
/// showed as a bar flashing across the top of the overlay on every snip.
/// Passing `-1` as `lParam` tells `DefWindowProc` not to repaint the
/// non-client area; everything else about the message (winit's focus
/// tracking, the return value) is unchanged.
pub fn suppress_caption_paint(window: &winit::window::Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    unsafe {
        SetWindowSubclass(
            h.hwnd.get() as HWND,
            Some(no_caption_proc),
            NO_CAPTION_ID,
            0,
        );
    }
}

const NO_CAPTION_ID: usize = 0x4B52_4142; // "KRAB"

unsafe extern "system" fn no_caption_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_NCACTIVATE => DefSubclassProc(hwnd, msg, wparam, -1),
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(no_caption_proc), NO_CAPTION_ID);
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
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

    /// Draw the mouse pointer onto the grabbed area at (`x`, `y`).
    fn draw_cursor(&mut self, x: i32, y: i32) {
        unsafe {
            let mut ci: CURSORINFO = zeroed();
            ci.cbSize = size_of::<CURSORINFO>() as u32;
            if GetCursorInfo(&mut ci) == 0 || ci.flags & CURSOR_SHOWING == 0 || ci.hCursor.is_null()
            {
                return;
            }
            let mut ii: ICONINFO = zeroed();
            if GetIconInfo(ci.hCursor, &mut ii) == 0 {
                return;
            }
            // GetIconInfo hands out copies of the cursor's bitmaps.
            if !ii.hbmMask.is_null() {
                DeleteObject(ii.hbmMask);
            }
            if !ii.hbmColor.is_null() {
                DeleteObject(ii.hbmColor);
            }
            DrawIconEx(
                self.mem,
                ci.ptScreenPos.x - ii.xHotspot as i32 - x,
                ci.ptScreenPos.y - ii.yHotspot as i32 - y,
                ci.hCursor,
                0,
                0,
                0,
                null_mut(),
                DI_NORMAL,
            );
            GdiFlush();
        }
    }

    /// Copy the screen area at (`x`, `y`) into `out` as `0xFFRRGGBB`.
    fn grab(&mut self, x: i32, y: i32, out: &mut Vec<u32>) -> Result<(), String> {
        self.grab_with(x, y, out, false)
    }

    fn grab_with(
        &mut self,
        x: i32,
        y: i32,
        out: &mut Vec<u32>,
        cursor: bool,
    ) -> Result<(), String> {
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
            if cursor {
                self.draw_cursor(x, y);
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
        list.push(WinInfo {
            rect,
            title,
            id: hwnd as usize as u64,
        });
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

/// [`capture_window`] works here.
pub const RENDERS_WINDOWS: bool = true;

/// Render one window by itself, without the windows on top of it.
///
/// `PrintWindow` with `PW_RENDERFULLCONTENT` copies the window's own DWM
/// surface, so it also works for GPU-drawn apps (browsers, Electron) and does
/// not care what covers the window - including our overlay.
pub fn capture_window(id: u64) -> Option<WindowImage> {
    let hwnd = id as usize as HWND;
    unsafe {
        // A hung window would never answer and freeze the overlay with it.
        if IsWindow(hwnd) == 0 || IsIconic(hwnd) != 0 || IsHungAppWindow(hwnd) != 0 {
            return None;
        }
        let mut wr: RECT = zeroed();
        if GetWindowRect(hwnd, &mut wr) == 0 {
            return None;
        }
        // The visible frame; GetWindowRect includes the invisible resize border.
        let mut fr = wr;
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut fr as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        );
        let blit = Blitter::new(wr.right - wr.left, wr.bottom - wr.top).ok()?;
        let n = (blit.w * blit.h) as usize;
        std::ptr::write_bytes(blit.bits as *mut u32, 0, n);
        if PrintWindow(hwnd, blit.mem, PW_RENDERFULLCONTENT) == 0 {
            return None;
        }
        GdiFlush();
        let frame = RectI::new(
            fr.left - wr.left,
            fr.top - wr.top,
            fr.right - fr.left,
            fr.bottom - fr.top,
        )
        .intersect(&RectI::new(0, 0, blit.w, blit.h));
        if frame.is_empty() {
            return None;
        }
        let src = std::slice::from_raw_parts(blit.bits, n);
        let mut px = Vec::with_capacity((frame.w * frame.h) as usize);
        for y in frame.y..frame.bottom() {
            let row = (y * blit.w) as usize;
            let line = &src[row + frame.x as usize..row + frame.right() as usize];
            px.extend(line.iter().map(|p| p | 0xFF00_0000));
        }
        let dpi = GetDpiForWindow(hwnd).max(96);
        let radius = (8 * dpi).div_ceil(96) as i32 + 1;
        clear_corners(&mut px, frame.w, frame.h, radius);
        Some(WindowImage {
            w: frame.w as u32,
            h: frame.h as u32,
            px,
        })
    }
}

/// Windows 11 rounds window corners when it composes the screen, so the
/// window's own surface may come back with black where the rounding cuts
/// away. Mark black pixels outside the corner arcs as not drawn, so the screen
/// as captured shows through there.
fn clear_corners(px: &mut [u32], w: i32, h: i32, r: i32) {
    if w < 2 * r || h < 2 * r {
        return;
    }
    let rf = r as f32;
    for dy in 0..r {
        for dx in 0..r {
            // Distance from the arc's centre, measured at the pixel centre.
            let (ox, oy) = (rf - dx as f32 - 0.5, rf - dy as f32 - 0.5);
            if ox * ox + oy * oy <= (rf - 1.0) * (rf - 1.0) {
                continue;
            }
            for (x, y) in [
                (dx, dy),
                (w - 1 - dx, dy),
                (dx, h - 1 - dy),
                (w - 1 - dx, h - 1 - dy),
            ] {
                let p = &mut px[(y * w + x) as usize];
                if *p & 0x00FF_FFFF == 0 {
                    *p = 0;
                }
            }
        }
    }
}

pub struct Grabber {
    blitter: Blitter,
    x: i32,
    y: i32,
    cursor: bool,
}

impl Grabber {
    pub fn new(
        _monitor: usize,
        monitor_native: RectI,
        local: RectI,
        cursor: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            blitter: Blitter::new(local.w, local.h)?,
            x: monitor_native.x + local.x,
            y: monitor_native.y + local.y,
            cursor,
        })
    }

    pub fn grab(&mut self, out: &mut Vec<u32>) -> Result<(), String> {
        self.blitter.grab_with(self.x, self.y, out, self.cursor)
    }
}

/// Whether a mouse button is held down right now.
pub fn mouse_down() -> bool {
    unsafe {
        (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0
            || (GetAsyncKeyState(VK_RBUTTON as i32) as u16 & 0x8000) != 0
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

/// The system clipboard, used directly for images: arboard PNG-encodes
/// every image before anything is on the clipboard, which takes a
/// noticeable moment for a full-screen snip.
pub mod clipboard {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard,
        RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows_sys::Win32::System::Ole::CF_DIBV5;

    /// Open the clipboard, retrying while another program holds it.
    fn open() -> Result<(), String> {
        for _ in 0..20 {
            if unsafe { OpenClipboard(null_mut()) } != 0 {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Err("the clipboard is in use by another program".into())
    }

    /// Hand `bytes` to the open clipboard as `format`.
    unsafe fn put(format: u32, len: usize, fill: impl FnOnce(&mut [u8])) -> Result<(), String> {
        let h = GlobalAlloc(GMEM_MOVEABLE, len);
        if h.is_null() {
            return Err("out of memory".into());
        }
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            GlobalFree(h);
            return Err("out of memory".into());
        }
        fill(std::slice::from_raw_parts_mut(p, len));
        GlobalUnlock(h);
        if SetClipboardData(format, h).is_null() {
            GlobalFree(h);
            return Err("could not put the image on the clipboard".into());
        }
        Ok(())
    }

    /// A BITMAPV5HEADER followed by bottom-up BGRA rows with straight alpha
    /// (bottom-up because Word and WordPad cannot paste top-down DIBs).
    pub fn dibv5(w: u32, h: u32, rgba: &[u8], out: &mut [u8]) {
        const HEADER: usize = 124;
        let put32 =
            |o: &mut [u8], at: usize, v: u32| o[at..at + 4].copy_from_slice(&v.to_le_bytes());
        out[..HEADER].fill(0);
        put32(out, 0, HEADER as u32);
        put32(out, 4, w);
        put32(out, 8, h); // positive: bottom-up
        out[12..14].copy_from_slice(&1u16.to_le_bytes()); // planes
        out[14..16].copy_from_slice(&32u16.to_le_bytes()); // bits
        put32(out, 16, 3); // BI_BITFIELDS
        put32(out, 20, w * h * 4);
        put32(out, 40, 0x00FF_0000);
        put32(out, 44, 0x0000_FF00);
        put32(out, 48, 0x0000_00FF);
        put32(out, 52, 0xFF00_0000);
        put32(out, 56, 0x7352_4742); // LCS_sRGB
        put32(out, 108, 4); // LCS_GM_IMAGES
        let row = (w * 4) as usize;
        for y in 0..h as usize {
            let src = &rgba[y * row..(y + 1) * row];
            let dst_y = h as usize - 1 - y;
            let dst = &mut out[HEADER + dst_y * row..HEADER + (dst_y + 1) * row];
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
            }
        }
    }

    /// Replace the clipboard with an image. Returns the clipboard's sequence
    /// number afterwards, to recognize it later.
    pub fn set_image(w: u32, h: u32, rgba: &[u8]) -> Result<u32, String> {
        open()?;
        let result = unsafe {
            if EmptyClipboard() == 0 {
                Err("could not clear the clipboard".to_string())
            } else {
                put(CF_DIBV5 as u32, 124 + rgba.len(), |b| dibv5(w, h, rgba, b))
            }
        };
        unsafe { CloseClipboard() };
        // Closing the clipboard counts as a change too: read the number after.
        let seq = unsafe { GetClipboardSequenceNumber() };
        result.map(|()| seq)
    }

    /// Add a PNG rendition to the clipboard, but only if it still holds the
    /// image put there at `seq` (the user may have copied something else).
    pub fn add_png(png: &[u8], seq: u32) -> bool {
        if open().is_err() {
            return false;
        }
        let ok = unsafe {
            if GetClipboardSequenceNumber() != seq {
                false
            } else {
                let name: Vec<u16> = "PNG\0".encode_utf16().collect();
                let format = RegisterClipboardFormatW(name.as_ptr());
                format != 0 && put(format, png.len(), |b| b.copy_from_slice(png)).is_ok()
            }
        };
        unsafe { CloseClipboard() };
        ok
    }
}

/// Turn the mouse wheel by `notches` (negative scrolls down) over the
/// screen point `at`. Returns whether this platform can scroll by itself;
/// zero notches only asks that.
pub fn scroll_wheel_at(at: (i32, i32), notches: i32) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;
    if notches == 0 {
        return true;
    }
    unsafe {
        // The wheel scrolls whatever is under the pointer.
        SetCursorPos(at.0, at.1);
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: (notches * 120) as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        SendInput(1, &input, size_of::<INPUT>() as i32) == 1
    }
}
