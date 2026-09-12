//! Win32 window setup for the embedded video output.
//!
//! Hand-rolled `user32`/`gdi32` bindings, in the same spirit as `mpv.rs`: a
//! handful of entry points, no binding crate to keep in step with the window
//! library. This module is only compiled on Windows (see `main.rs`).
//!
//! Two things have to be true of the host window for mpv to render into it:
//!
//! * It must never paint itself. winit registers its window class without a
//!   background brush, so a window that nothing has painted yet shows as a
//!   plain white rectangle — the "white window" users report as a crash. mpv's
//!   video output paints the window, and until it does we want black.
//! * It must have `WS_CLIPCHILDREN`. mpv renders into a *child* window covering
//!   our client area, and without that style our own paints draw over it, so
//!   the picture vanishes. winit 0.29 does not set the bit; it arrived as an
//!   opt-in attribute in later versions.

use std::ffi::{c_int, c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use winit::window::Window;

pub type Hwnd = *mut c_void;

#[link(name = "user32")]
extern "system" {
    fn IsWindow(hwnd: Hwnd) -> c_int;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: c_int) -> isize;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: c_int, value: isize) -> isize;
    fn SetClassLongPtrW(hwnd: Hwnd, index: c_int, value: isize) -> isize;
    fn InvalidateRect(hwnd: Hwnd, rect: *const c_void, erase: c_int) -> c_int;
    fn UpdateWindow(hwnd: Hwnd) -> c_int;
    fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, flags: u32) -> c_int;
}

#[link(name = "gdi32")]
extern "system" {
    fn GetStockObject(index: c_int) -> *mut c_void;
}

const GWL_STYLE: c_int = -16;
const GCLP_HBRBACKGROUND: c_int = -10;
const WS_CLIPCHILDREN: isize = 0x0200_0000;
const BLACK_BRUSH: c_int = 4;

const MB_OK: u32 = 0x0000_0000;
const MB_ICONERROR: u32 = 0x0000_0010;
const MB_SETFOREGROUND: u32 = 0x0001_0000;

/// The window handle mpv should embed itself into.
pub fn hwnd_of(window: &Window) -> Result<Hwnd, String> {
    match window.raw_window_handle() {
        RawWindowHandle::Win32(h) => {
            let hwnd = h.hwnd as Hwnd;
            if hwnd.is_null() {
                return Err("the player window has no Win32 handle yet".into());
            }
            Ok(hwnd)
        }
        other => Err(format!("unsupported window system: {other:?}")),
    }
}

/// Make the host window behave like a video surface: black, and never painting
/// over its children.
pub fn prepare_video_window(hwnd: Hwnd) -> Result<(), String> {
    if unsafe { IsWindow(hwnd) } == 0 {
        return Err("the player window handle is not a valid window".into());
    }

    unsafe {
        // 1. Black background. Anything the window paints before the video
        //    output has a frame — including the case where it never gets one —
        //    is now black instead of white.
        SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, GetStockObject(BLACK_BRUSH) as isize);

        // 2. Keep our own painting out of the video child window's way.
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        if style & WS_CLIPCHILDREN == 0 {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_CLIPCHILDREN);
        }

        // 3. Paint now, so the first frame the user sees is already correct.
        InvalidateRect(hwnd, ptr::null(), 1);
        UpdateWindow(hwnd);
    }

    Ok(())
}

/// A dialog box. Used from the panic hook, so it must not depend on anything
/// that can itself fail (no COM, no dialogs from a half-initialised state).
pub fn message_box(title: &str, text: &str) {
    let caption = wide(title);
    let body = wide(text);
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            body.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_null_terminated_for_win32() {
        let w = wide("hi");
        assert_eq!(w, vec![b'h' as u16, b'i' as u16, 0]);
    }
}
