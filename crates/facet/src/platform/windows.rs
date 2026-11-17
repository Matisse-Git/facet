use std::ffi::{c_void, OsString};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::HSTRING;
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmDefWindowProc, DwmExtendFrameIntoClientArea, DwmGetWindowAttribute, DwmSetWindowAttribute,
    DWMSBT_MAINWINDOW, DWMWA_CAPTION_BUTTON_BOUNDS, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::Graphics::Gdi::{
    FillRect, GetStockObject, RedrawWindow, ScreenToClient, BLACK_BRUSH, HBRUSH, HDC, RDW_ERASE,
    RDW_INVALIDATE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, GetClientRect, GetWindowPlacement, IsZoomed, SetWindowLongPtrW,
    SetWindowPlacement, SetWindowPos, GWLP_WNDPROC, HTCAPTION, HTCLIENT, HTTOP, HTTOPLEFT,
    HTTOPRIGHT, NCCALCSIZE_PARAMS, SM_CXPADDEDBORDER, SM_CYSIZEFRAME, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWMAXIMIZED,
    SW_SHOWMINIMIZED, WINDOWPLACEMENT, WM_ERASEBKGND, WM_NCCALCSIZE, WM_NCHITTEST, WNDPROC,
};

/// A rectangle in physical pixels, relative to the window's client area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PxRect {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Saved window position: the *restored* (non-maximized) outer rectangle, plus whether the
/// window was maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

/// What the window procedure needs to know about the title bar. Written by the UI every frame
/// it changes, read during `WM_NCHITTEST` (same thread, but the lock keeps it simple and safe).
struct TitleBarHit {
    /// Height of the draggable title bar, in physical pixels from the top of the client area.
    height: i32,
    /// Regions inside the title bar that are real controls (tabs, buttons) and must get clicks.
    interactive: Vec<PxRect>,
}

static TITLE_BAR: Mutex<TitleBarHit> = Mutex::new(TitleBarHit {
    height: 0,
    interactive: Vec::new(),
});

/// The window procedure we replaced (winit's), stored as an integer so it can live in a static.
static PREVIOUS_WNDPROC: AtomicIsize = AtomicIsize::new(0);

/// Handle to Facet's main window with the custom chrome installed.
#[derive(Debug, Clone, Copy)]
pub struct Chrome {
    hwnd: isize,
}

impl Chrome {
    /// Turn a standard window into Facet's window: Mica backdrop behind a transparent client
    /// area, no system title bar, but real DWM-drawn caption buttons and native resize/snap.
    ///
    /// Returns `None` if the handle is not a Win32 window or the chrome was already installed.
    pub fn install(window: &impl HasWindowHandle, dark: bool) -> Option<Self> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return None;
        };
        let chrome = Self {
            hwnd: handle.hwnd.get(),
        };
        let hwnd = chrome.hwnd();

        // SAFETY: `hwnd` is the live top-level window owned by this thread. The attribute
        // pointers reference locals that outlive the calls.
        unsafe {
            // Let the DWM frame (and with it the Mica material and the caption buttons) extend
            // under the entire client area. Wherever we render transparent pixels, it shows.
            let margins = MARGINS {
                cxLeftWidth: -1,
                cxRightWidth: -1,
                cyTopHeight: -1,
                cyBottomHeight: -1,
            };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

            let backdrop = DWMSBT_MAINWINDOW;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                std::ptr::from_ref(&backdrop).cast::<c_void>(),
                size_of_val(&backdrop) as u32,
            );
        }
        chrome.set_dark(dark);

        // SAFETY: we install a window procedure with the required signature and keep the
        // previous one to forward every message we do not handle ourselves.
        unsafe {
            let previous = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, window_proc as *const () as isize);
            if previous == 0 || previous == window_proc as *const () as isize {
                return None;
            }
            PREVIOUS_WNDPROC.store(previous, Ordering::Release);

            // Make Windows re-query the non-client area now that we answer WM_NCCALCSIZE.
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            // Erase the GDI surface once with our WM_ERASEBKGND handler (see `window_proc`).
            let _ = RedrawWindow(Some(hwnd), None, None, RDW_ERASE | RDW_INVALIDATE);
        }
        Some(chrome)
    }

    fn hwnd(self) -> HWND {
        HWND(self.hwnd as *mut c_void)
    }

    /// The raw handle as an integer, so it can cross threads (e.g. to own a dialog).
    pub fn raw(self) -> isize {
        self.hwnd
    }

    /// Switch the DWM frame (caption button glyphs, Mica tint) between dark and light.
    pub fn set_dark(self, dark: bool) {
        let value = i32::from(dark);
        // SAFETY: valid window handle; the pointer references a local i32 (a Win32 BOOL).
        unsafe {
            let _ = DwmSetWindowAttribute(
                self.hwnd(),
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                std::ptr::from_ref(&value).cast::<c_void>(),
                size_of_val(&value) as u32,
            );
        }
    }

    /// Bounds of the DWM-drawn caption buttons (minimize/maximize/close), in physical pixels
    /// relative to the window. `None` if DWM cannot report them.
    pub fn caption_buttons(self) -> Option<PxRect> {
        let mut rect = RECT::default();
        // SAFETY: valid window handle; the out pointer references a local RECT of the size given.
        let result = unsafe {
            DwmGetWindowAttribute(
                self.hwnd(),
                DWMWA_CAPTION_BUTTON_BOUNDS,
                std::ptr::from_mut(&mut rect).cast::<c_void>(),
                size_of::<RECT>() as u32,
            )
        };
        (result.is_ok() && rect.right > rect.left).then_some(PxRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }

    /// Tell the window procedure where the draggable title bar is and which parts of it are
    /// interactive controls. All values are physical pixels in client coordinates.
    pub fn set_title_bar(self, height: i32, interactive: &[PxRect]) {
        let mut state = lock_title_bar();
        state.height = height;
        if state.interactive != interactive {
            state.interactive.clear();
            state.interactive.extend_from_slice(interactive);
        }
    }

    /// Current window placement, for saving. `None` while minimized state can't be read.
    pub fn placement(self) -> Option<Placement> {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: valid window handle and a correctly sized out structure.
        unsafe { GetWindowPlacement(self.hwnd(), &mut placement).ok()? };
        let rect = placement.rcNormalPosition;
        // SAFETY: valid window handle.
        let zoomed = unsafe { IsZoomed(self.hwnd()).as_bool() };
        let maximized = zoomed
            || (placement.showCmd == SW_SHOWMINIMIZED.0 as u32
                && placement.flags.0 & WPF_RESTORETOMAXIMIZED != 0);
        Some(Placement {
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
            maximized,
        })
    }

    /// Restore a saved placement. Windows clamps it onto a visible monitor if the display
    /// layout changed. Call before the window is first shown.
    pub fn restore_placement(self, saved: Placement) {
        if saved.width < 200 || saved.height < 150 {
            return;
        }
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: valid window handle and correctly sized structures.
        unsafe {
            if GetWindowPlacement(self.hwnd(), &mut placement).is_err() {
                return;
            }
            placement.rcNormalPosition = RECT {
                left: saved.x,
                top: saved.y,
                right: saved.x + saved.width,
                bottom: saved.y + saved.height,
            };
            // A maximized window has to be shown to become maximized; a normal one stays
            // hidden until the first frame is ready.
            placement.showCmd = if saved.maximized {
                SW_SHOWMAXIMIZED.0 as u32
            } else {
                SW_HIDE.0 as u32
            };
            let _ = SetWindowPlacement(self.hwnd(), &placement);
        }
    }
}

const WPF_RESTORETOMAXIMIZED: u32 = 0x0002;

fn lock_title_bar() -> std::sync::MutexGuard<'static, TitleBarHit> {
    TITLE_BAR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Thickness of the invisible resize border at the window's current DPI.
///
/// # Safety
/// `hwnd` must be a valid window handle.
unsafe fn resize_border(hwnd: HWND) -> i32 {
    // SAFETY: guaranteed by the caller.
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}

/// Facet's window procedure, layered over winit's.
///
/// * `WM_NCCALCSIZE`: remove the system title bar but keep the standard resize borders.
/// * `WM_NCHITTEST`: caption buttons are DWM's; the empty title bar drags the window; the top
///   edge resizes; everything else is client area for egui.
///
/// # Safety
/// Called by Windows with a valid window handle and message parameters.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: `PREVIOUS_WNDPROC` holds the non-null procedure returned by SetWindowLongPtrW;
    // an integer of pointer size has the same layout as the `Option<fn>` it came from.
    let previous: WNDPROC =
        unsafe { std::mem::transmute::<isize, WNDPROC>(PREVIOUS_WNDPROC.load(Ordering::Acquire)) };
    // SAFETY: forwarding the message we were given to the procedure we replaced.
    let forward = || unsafe { CallWindowProcW(previous, hwnd, msg, wparam, lparam) };

    // DWM owns the caption buttons: hit-testing, hover, press and the snap-layouts flyout.
    let mut dwm_result = LRESULT(0);
    // SAFETY: valid handle and message; the out pointer references a local.
    if unsafe { DwmDefWindowProc(hwnd, msg, wparam, lparam, &mut dwm_result) }.as_bool() {
        return dwm_result;
    }

    match msg {
        WM_ERASEBKGND => {
            // The window's GDI surface sits between the DWM frame (Mica, caption buttons) and
            // our swapchain. GDI black has zero alpha, so painting it black makes the surface
            // invisible and lets the frame show wherever we render transparent pixels.
            let mut client = RECT::default();
            // SAFETY: valid handle; for WM_ERASEBKGND wparam is the device context to paint.
            unsafe {
                let _ = GetClientRect(hwnd, &mut client);
                FillRect(
                    HDC(wparam.0 as *mut c_void),
                    &client,
                    HBRUSH(GetStockObject(BLACK_BRUSH).0),
                );
            }
            LRESULT(1)
        }
        WM_NCCALCSIZE if wparam.0 != 0 => {
            let params = lparam.0 as *mut NCCALCSIZE_PARAMS;
            // SAFETY: for WM_NCCALCSIZE with wparam=TRUE, lparam points to NCCALCSIZE_PARAMS.
            let original_top = unsafe { (*params).rgrc[0].top };
            let result = forward();
            if result.0 != 0 {
                return result;
            }
            // The default handler reserved a title bar; give that strip back to the client
            // area. A maximized window overhangs the monitor by the border thickness, so keep
            // that much reserved or the top of our UI would be cut off.
            // SAFETY: valid handle; `params` is still the structure Windows passed in.
            unsafe {
                let inset = if IsZoomed(hwnd).as_bool() {
                    resize_border(hwnd)
                } else {
                    0
                };
                (*params).rgrc[0].top = original_top + inset;
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let hit = forward();
            if hit.0 as u32 != HTCLIENT {
                return hit;
            }
            // Screen coordinates are signed 16-bit values packed into lparam.
            let mut point = POINT {
                x: i32::from((lparam.0 & 0xFFFF) as u16 as i16),
                y: i32::from(((lparam.0 >> 16) & 0xFFFF) as u16 as i16),
            };
            let mut client = RECT::default();
            // SAFETY: valid handle; out pointers reference locals.
            let (zoomed, border) = unsafe {
                let _ = ScreenToClient(hwnd, &mut point);
                let _ = GetClientRect(hwnd, &mut client);
                (IsZoomed(hwnd).as_bool(), resize_border(hwnd))
            };

            if !zoomed && point.y < border {
                let corner = border * 2;
                let code = if point.x < corner {
                    HTTOPLEFT
                } else if point.x >= client.right - corner {
                    HTTOPRIGHT
                } else {
                    HTTOP
                };
                return LRESULT(code as isize);
            }

            let title_bar = lock_title_bar();
            let in_title_bar = point.y >= 0 && point.y < title_bar.height;
            let on_control = title_bar
                .interactive
                .iter()
                .any(|rect| rect.contains(point.x, point.y));
            if in_title_bar && !on_control {
                LRESULT(HTCAPTION as isize)
            } else {
                LRESULT(HTCLIENT as isize)
            }
        }
        _ => forward(),
    }
}

/// Show the native "pick a folder" dialog, modal to `owner`. **Blocks** until the user is
/// done, so call it from a dedicated thread, never the UI thread.
///
/// `Ok(None)` means the user cancelled.
pub fn pick_folder(owner: Option<isize>, title: &str) -> Result<Option<PathBuf>, String> {
    // SAFETY: standard COM usage on a thread we initialise as a single-threaded apartment for
    // the duration of the call; all interface pointers are owned by `windows` smart pointers.
    unsafe {
        let initialized =
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let result = (|| -> windows::core::Result<Option<PathBuf>> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let options = dialog.GetOptions()?;
            dialog
                .SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
            dialog.SetTitle(&HSTRING::from(title))?;

            let owner = owner.map(|raw| HWND(raw as *mut c_void));
            if let Err(error) = dialog.Show(owner) {
                return if error.code() == ERROR_CANCELLED.to_hresult() {
                    Ok(None)
                } else {
                    Err(error)
                };
            }

            let item = dialog.GetResult()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = PathBuf::from(OsString::from_wide(name.as_wide()));
            CoTaskMemFree(Some(name.0.cast::<c_void>().cast_const()));
            Ok(Some(path))
        })();
        if initialized {
            CoUninitialize();
        }
        result.map_err(|error| error.message())
    }
}

/// Open a folder in File Explorer. Spawns and returns immediately.
pub fn reveal_in_explorer(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// The local time zone's current offset from UTC, in seconds (east of UTC is positive).
pub fn local_utc_offset_seconds() -> i64 {
    const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
    let mut info = TIME_ZONE_INFORMATION::default();
    // SAFETY: the out pointer references a local structure.
    let id = unsafe { GetTimeZoneInformation(&mut info) };
    let bias_minutes = info.Bias
        + if id == TIME_ZONE_ID_DAYLIGHT {
            info.DaylightBias
        } else {
            info.StandardBias
        };
    -i64::from(bias_minutes) * 60
}
