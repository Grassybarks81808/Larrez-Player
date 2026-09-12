//! The on-screen interface: a floating transport bar and a playlist panel.
//!
//! Why it is built this way
//! ------------------------
//! The video is not ours to draw on. libmpv is handed the window's `HWND` and
//! renders into a *child* window covering the whole client area, and the parent
//! carries `WS_CLIPCHILDREN` so our own painting cannot reach over it. Anything
//! drawn into the main window sits behind the picture.
//!
//! So the chrome is two layered child windows of its own, kept above mpv's
//! child in the sibling z-order and made transparent with a colour key: the
//! surface is filled with the key colour, everything the user should see is
//! painted solid on top, and keyed pixels are where the video shows through.
//! Keyed pixels are transparent to hit testing as well, so a click in the
//! rounded corner of the bar falls through to the video instead of being eaten.
//!
//! This keeps the promise the crate is built on — no per-frame work. The bar
//! repaints on pointer input and on the 4 Hz tick that already exists, and only
//! while the chrome is visible; when it has faded out, nothing paints at all.
//! The idle countdown and the fade are driven by `WM_TIMER` on the chrome
//! itself, so hiding the interface costs the main loop nothing.
//!
//! Geometry is derived from each window's client rect at paint time and scaled
//! by its DPI, so a resize cannot leave a control where the pointer no longer
//! agrees with the pixels, and a 4K panel gets a legible bar without a manifest.
//!
//! Two invariants in this file are load-bearing and easy to break by accident:
//!
//! * **No re-entrance.** The state lives in one mutex. Everything that runs
//!   inside a window procedure takes that lock, and `CreateWindowExW`,
//!   `SetWindowPos` and `ShowWindow` dispatch *some* messages to the target
//!   window synchronously — so the procedure must not lock for those. It only
//!   ever locks for the messages it actually handles (see `wnd_proc`), and
//!   nothing that can pump a message (`place`, `ShowWindow` on creation) is
//!   called while the lock is held by the main loop for any reason that matters.
//! * **GDI restoration.** Every `SelectObject` has a matching restore before the
//!   object is deleted. The helpers at the bottom of the file are small on
//!   purpose so that rule is checkable by eye.
//!
//! All of it is Win32. On other platforms the same API answers with no-ops, so
//! the crate still builds and its tests still run.

use std::ffi::c_void;

/// The handle type the chrome is handed. Same underlying type as `win::Hwnd`;
/// kept local so this module does not depend on a Windows-only one.
pub type Hwnd = *mut c_void;

/// Everything the chrome needs in order to draw itself. Published from the main
/// loop's tick — the interface never asks mpv for anything itself.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Snapshot {
    /// Playing position, seconds.
    pub pos: f64,
    /// Duration, seconds. Zero until mpv knows it.
    pub dur: f64,
    /// Fraction of the file already demuxed, 0.0..=1.0.
    pub buffered: f64,
    /// 0.0..=100.0, in mpv's units.
    pub volume: f64,
    pub speed: f64,
    /// A file is loaded and playing: the chrome is allowed to fade away.
    pub playing: bool,
    pub muted: bool,
    pub fullscreen: bool,
}

/// What the user asked the player to do, drained once per tick. Input never
/// reaches into mpv from inside a window procedure.
#[derive(Clone, Copy, Default, Debug)]
pub struct Actions {
    pub toggle_pause: bool,
    pub prev: bool,
    pub next: bool,
    pub seek: Option<f64>,
    pub volume: Option<f64>,
    pub toggle_mute: bool,
    pub cycle_speed: bool,
    pub toggle_fullscreen: bool,
    pub screenshot: bool,
    pub toggle_playlist: bool,
    pub play_row: Option<usize>,
    pub open_files: bool,
}

impl Actions {
    /// Whether anything at all was asked for, so the main loop can skip the
    /// whole apply path on a quiet tick.
    pub fn any(&self) -> bool {
        self.toggle_pause
            || self.prev
            || self.next
            || self.seek.is_some()
            || self.volume.is_some()
            || self.toggle_mute
            || self.cycle_speed
            || self.toggle_fullscreen
            || self.screenshot
            || self.toggle_playlist
            || self.play_row.is_some()
            || self.open_files
    }
}

#[cfg(windows)]
mod platform {
    use super::{Actions, Hwnd, Snapshot};
    use crate::playlist::fmt_time;
    use std::ffi::{c_int, c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

    // --- FFI ---------------------------------------------------------------
    // Hand-rolled as in win.rs and mpv.rs: the entry points we need and no
    // binding crate to keep in step with the window library. `LRESULT` is
    // LONG_PTR and `WPARAM`/`LPARAM` are its pointer-sized twins, hence
    // isize/usize. Which library a symbol lives in matters: a wrong `#[link]`
    // is an unresolved external, not a type error.
    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassExW(cls: *const WndClassEx) -> u16;
        fn CreateWindowExW(
            ex_style: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: c_int,
            y: c_int,
            w: c_int,
            h: c_int,
            parent: Hwnd,
            menu: Hwnd,
            instance: Hwnd,
            param: *const c_void,
        ) -> Hwnd;
        fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn DestroyWindow(hwnd: Hwnd) -> c_int;
        fn IsWindow(hwnd: Hwnd) -> c_int;
        fn SetLayeredWindowAttributes(hwnd: Hwnd, key: u32, alpha: u8, flags: u32) -> c_int;
        fn ShowWindow(hwnd: Hwnd, cmd: c_int) -> c_int;
        fn SetWindowPos(
            hwnd: Hwnd,
            after: Hwnd,
            x: c_int,
            y: c_int,
            w: c_int,
            h: c_int,
            flags: u32,
        ) -> c_int;
        fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> c_int;
        fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: c_int) -> c_int;
        fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Hwnd;
        fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> c_int;
        fn SetTimer(hwnd: Hwnd, id: usize, elapsed: u32, func: *const c_void) -> usize;
        fn KillTimer(hwnd: Hwnd, id: usize) -> c_int;
        fn SetCapture(hwnd: Hwnd) -> Hwnd;
        fn ReleaseCapture() -> c_int;
        fn TrackMouseEvent(evt: *mut TrackMouseEvent) -> c_int;
        fn LoadCursorW(instance: Hwnd, name: *const u16) -> Hwnd;
        fn GetModuleHandleW(name: *const u16) -> Hwnd;
        fn GetDpiForWindow(hwnd: Hwnd) -> u32;
        fn DrawTextW(hdc: Hwnd, text: *const u16, count: c_int, rect: *mut Rect, flags: u32) -> c_int;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateCompatibleDC(hdc: Hwnd) -> Hwnd;
        fn CreateCompatibleBitmap(hdc: Hwnd, w: c_int, h: c_int) -> Hwnd;
        fn SelectObject(hdc: Hwnd, obj: Hwnd) -> Hwnd;
        fn DeleteObject(obj: Hwnd) -> c_int;
        fn DeleteDC(hdc: Hwnd) -> c_int;
        fn CreateSolidBrush(color: u32) -> Hwnd;
        fn CreatePen(style: c_int, width: c_int, color: u32) -> Hwnd;
        fn CreateFontW(
            height: c_int,
            width: c_int,
            escapement: c_int,
            orientation: c_int,
            weight: c_int,
            italic: c_int,
            underline: c_int,
            strikeout: c_int,
            charset: c_int,
            out_precision: c_int,
            clip_precision: c_int,
            quality: c_int,
            pitch_and_family: c_int,
            face: *const u16,
        ) -> Hwnd;
        fn GetStockObject(index: c_int) -> Hwnd;
        fn BitBlt(
            dst: Hwnd,
            x: c_int,
            y: c_int,
            w: c_int,
            h: c_int,
            src: Hwnd,
            sx: c_int,
            sy: c_int,
            raster: u32,
        ) -> c_int;
        fn FillRect(hdc: Hwnd, rect: *const Rect, brush: Hwnd) -> c_int;
        fn RoundRect(hdc: Hwnd, l: c_int, t: c_int, r: c_int, b: c_int, w: c_int, h: c_int) -> c_int;
        fn Rectangle(hdc: Hwnd, l: c_int, t: c_int, r: c_int, b: c_int) -> c_int;
        fn Ellipse(hdc: Hwnd, l: c_int, t: c_int, r: c_int, b: c_int) -> c_int;
        fn Polygon(hdc: Hwnd, pts: *const Point, count: c_int) -> c_int;
        fn Polyline(hdc: Hwnd, pts: *const Point, count: c_int) -> c_int;
        fn SetTextColor(hdc: Hwnd, color: u32) -> u32;
        fn SetBkMode(hdc: Hwnd, mode: c_int) -> c_int;
        fn GetTextExtentPoint32W(hdc: Hwnd, text: *const u16, count: c_int, size: *mut Size) -> c_int;
        fn ExtTextOutW(
            hdc: Hwnd,
            x: c_int,
            y: c_int,
            options: u32,
            rect: *const Rect,
            text: *const u16,
            count: c_int,
        ) -> c_int;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    // --- Win32 constants ---------------------------------------------------
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_EX_LAYERED: u32 = 0x0008_0000;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const WS_EX_NOPARENTNOTIFY: u32 = 0x0000_0004;
    const LWA_COLORKEY: u32 = 0x0000_0001;
    const LWA_ALPHA: u32 = 0x0000_0002;
    const SW_HIDE: c_int = 0;
    const SW_SHOWNA: c_int = 8;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const CS_DROPSHADOW: u32 = 0x0002_0000;
    const HWND_TOP: Hwnd = std::ptr::null_mut();
    const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

    // Only these reach the state. Everything else goes straight to
    // DefWindowProc — see the re-entrance note at the top of the file.
    const WM_PAINT: u32 = 0x000F;
    const WM_ERASEBKGND: u32 = 0x0014;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_MOUSEWHEEL: u32 = 0x020A;
    const WM_MOUSELEAVE: u32 = 0x02A3;
    const WM_TIMER: u32 = 0x0113;
    const IDC_ARROW: usize = 32512;

    const TRANSPARENT: c_int = 1;
    const NULL_BRUSH: c_int = 33;
    const PS_SOLID: c_int = 0;
    const SRCCOPY: u32 = 0x00CC_0020;
    const FW_SEMIBOLD: c_int = 600;
    const FW_NORMAL: c_int = 400;
    const DEFAULT_CHARSET: c_int = 1;
    const CLEARTYPE_QUALITY: c_int = 5;
    const VARIABLE_PITCH: c_int = 0x0010;
    const FF_DONTCARE: c_int = 0x0200;

    const TME_LEAVE: u32 = 0x0000_0002;
    const HOVER_DEFAULT: u32 = 0xFFFF_FFFF;
    const DT_LEFT: u32 = 0x0000;
    const DT_VCENTER: u32 = 0x0004;
    const DT_SINGLELINE: u32 = 0x0020;
    const DT_NOPREFIX: u32 = 0x2000;
    const DT_END_ELLIPSIS: u32 = 0x8000;

    // WM_TIMER carries the id in wParam and nothing else, so that is how the
    // two timers are told apart. Both live on the bar window and drive the
    // panel too, because the bar is the one that is always there.
    const TIMER_IDLE: usize = 1;
    const TIMER_FADE: usize = 2;
    const IDLE_MS: u32 = 2600;
    const FADE_MS: u32 = 33;
    /// Fully opaque is not available to a colour-keyed window and would also
    /// look wrong over a bright picture; ~93% reads as glass.
    const ALPHA_SOLID: u8 = 238;
    const ALPHA_STEP: c_int = 60;

    // Geometry, in logical pixels; scaled by DPI at use.
    const MARGIN: c_int = 16;
    const BAR_H: c_int = 82;
    const SCRUB_BAND: c_int = 24;
    const PAD: c_int = 13;
    const GAP: c_int = 6;
    const BTN: c_int = 40;
    const BTN_PLAY: c_int = 46;
    const TIME_W: c_int = 118;
    const VOL_W: c_int = 86;
    const CHIP_W: c_int = 56;
    const PANEL_W: c_int = 312;
    const ROW_H: c_int = 42;
    const HEAD_H: c_int = 44;
    /// Below this width the read-out and the speed chip give the buttons room,
    /// so every control stays reachable in a small window.
    const COMPACT_W: c_int = 720;

    // Palette. The key colour must never appear in what we draw or that pixel
    // turns see-through, so nothing here is pure black.
    const KEY: u32 = 0x0000_0000;
    const C_TEXT: u32 = rgb(0xE8, 0xEC, 0xF4);
    const C_MUTED: u32 = rgb(0x8B, 0x93, 0xA7);
    const C_ACCENT: u32 = rgb(0xFF, 0x8A, 0x3D);
    const C_ON_ACCENT: u32 = rgb(0x1A, 0x0E, 0x04);
    const C_PILL: u32 = rgb(0x14, 0x17, 0x1E);
    const C_LINE: u32 = rgb(0x2A, 0x31, 0x40);
    const C_HOVER: u32 = rgb(0x24, 0x2A, 0x36);
    const C_PRESS: u32 = rgb(0x33, 0x3B, 0x4C);
    const C_TRACK: u32 = rgb(0x39, 0x41, 0x53);
    const C_BUFFER: u32 = rgb(0x6B, 0x54, 0x3C);

    /// COLORREF is 0x00BBGGRR; building it from R, G, B names removes the
    /// temptation to write a literal backwards.
    const fn rgb(r: u8, g: u8, b: u8) -> u32 {
        (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
    }

    // --- Win32 structures --------------------------------------------------
    // repr(C) reproduces the C layout including padding, so the `cbSize` we
    // hand to Win32 is the one it expects on x64.
    #[repr(C)]
    #[derive(Clone, Copy, Default, PartialEq)]
    struct Point {
        x: c_int,
        y: c_int,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default, PartialEq)]
    struct Size {
        cx: c_int,
        cy: c_int,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default, PartialEq)]
    struct Rect {
        l: c_int,
        t: c_int,
        r: c_int,
        b: c_int,
    }

    impl Rect {
        const fn new(l: c_int, t: c_int, r: c_int, b: c_int) -> Self {
            Rect { l, t, r, b }
        }

        fn contains(self, p: Point) -> bool {
            p.x >= self.l && p.x < self.r && p.y >= self.t && p.y < self.b
        }

        fn w(self) -> c_int {
            self.r - self.l
        }

        fn h(self) -> c_int {
            self.b - self.t
        }

        fn cx(self) -> c_int {
            (self.l + self.r) / 2
        }

        fn cy(self) -> c_int {
            (self.t + self.b) / 2
        }

        /// Fraction `x` of the way across, clamped — the scrub and the volume
        /// slider are the same problem with different labels.
        fn frac_x(self, x: c_int) -> f64 {
            if self.w() <= 0 {
                return 0.0;
            }
            ((x - self.l) as f64 / self.w() as f64).clamp(0.0, 1.0)
        }
    }

    /// BeginPaint writes it and EndPaint reads it; our side only needs the layout to be right (asserted in the tests below).
    #[repr(C)]
    #[allow(dead_code)] // fields are written for Win32, never read back
    struct PaintStruct {
        hdc: Hwnd,
        erase: c_int,
        update: Rect,
        restore: c_int,
        inc_update: c_int,
        reserved: [u8; 32],
    }

    /// TrackMouseEvent() consumes this; nothing here reads it back.
    #[repr(C)]
    #[allow(dead_code)] // fields are written for Win32, never read back
    struct TrackMouseEvent {
        size: u32,
        flags: u32,
        hwnd_track: Hwnd,
        hover_time: u32,
    }

    /// RegisterClassExW reads every one of these; the struct exists to describe the layout, not to be queried.
    #[repr(C)]
    #[allow(dead_code)] // fields are written for Win32, never read back
    struct WndClassEx {
        size: u32,
        style: u32,
        wnd_proc: Option<unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize>,
        class_extra: c_int,
        window_extra: c_int,
        instance: Hwnd,
        icon: Hwnd,
        cursor: Hwnd,
        background: Hwnd,
        menu_name: *const u16,
        class_name: *const u16,
        small_icon: Hwnd,
    }

    // --- Model -------------------------------------------------------------
    /// A control the pointer can be over. `Scrub` and `Volume` are dragged
    /// rather than clicked; playlist rows are tracked separately because there
    /// are many of them.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Hit {
        Prev,
        Rewind,
        Play,
        Forward,
        Next,
        Scrub,
        Mute,
        Volume,
        Speed,
        List,
        Snap,
        Full,
        Open,
        Close,
    }

    impl Hit {
        /// Dragged rather than pressed.
        fn is_slider(self) -> bool {
            matches!(self, Hit::Scrub | Hit::Volume)
        }
    }

    /// Where the controls are, for the bar's current width. Recomputed on every
    /// paint and every hit test from the live client rect: a few integer ops, in
    /// exchange for a layout that cannot go stale.
    #[derive(Default)]
    struct Layout {
        pill: Rect,
        scrub: Rect,
        volume: Rect,
        time: Rect,
        hits: Vec<(Hit, Rect)>,
        compact: bool,
        scale: c_int,
    }

    impl Layout {
        fn at(&self, p: Point) -> Option<(Hit, Rect)> {
            self.hits.iter().find(|(_, r)| r.contains(p)).copied()
        }
    }

    /// The panel's parts, likewise derived from its own client rect.
    struct Panel {
        head: Rect,
        open: Rect,
        close: Rect,
        body_top: c_int,
        scale: c_int,
    }

    #[derive(Default)]
    struct Chrome {
        parent: Wrap,
        bar: Wrap,
        panel: Wrap,
        snap: Snapshot,
        actions: Actions,
        rows: Vec<String>,
        current: Option<usize>,
        /// Panel open/closed. It lives here so the panel can fade with the bar
        /// without a round trip through the player.
        list_open: bool,
        visible: bool,
        alpha: u8,
        fading_out: bool,
        hot: Option<Hit>,
        pressed: Option<Hit>,
        hot_row: Option<usize>,
        dragging: bool,
        hover_armed: bool,
        scroll: c_int,
        bar_rect: Rect,
        panel_rect: Rect,
        registered: bool,
    }

    /// An HWND is a pointer and `Mutex` wants its contents to be `Send`. The
    /// chrome is driven from the window thread only — by the main loop, and by
    /// the window procedure running inside that loop's message pump. This
    /// newtype states that constraint instead of quietly widening access.
    #[derive(Clone, Copy, Default)]
    struct Wrap(Hwnd);

    unsafe impl Send for Wrap {}

    impl Wrap {
        fn get(self) -> Hwnd {
            self.0
        }

        /// A window can be destroyed underneath us while the parent closes, and
        /// a late tick must not poke a dead handle.
        fn live(self) -> bool {
            !self.0.is_null() && unsafe { IsWindow(self.0) } != 0
        }
    }

    static CHROME: OnceLock<Mutex<Chrome>> = OnceLock::new();

    fn lock() -> MutexGuard<'static, Chrome> {
        CHROME
            .get_or_init(|| Mutex::new(Chrome::default()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    // --- Public entry points ----------------------------------------------
    /// Creates the chrome windows, or re-asserts their z-order if they exist.
    /// mpv recreates its video child as files change, and a sibling that ends up
    /// beneath it is a control bar nobody can click.
    pub fn attach(parent: Hwnd) -> Result<(), String> {
        let mut c = lock();
        c.parent = Wrap(parent);
        if parent.is_null() {
            return Err("no window to attach the interface to".into());
        }
        unsafe { create_chrome(&mut c) }
    }

    pub fn dispose() {
        // Detach first, destroy after the lock is gone: DestroyWindow dispatches
        // to the window procedure, which takes that lock.
        let (bar, panel) = {
            let mut c = lock();
            let handles = (c.bar, c.panel);
            c.bar = Wrap::default();
            c.panel = Wrap::default();
            c.visible = false;
            handles
        };
        unsafe {
            for w in [bar, panel] {
                if w.live() {
                    DestroyWindow(w.get());
                }
            }
        }
    }

    /// Publishes playback state. Only the values the bar draws are compared, and
    /// a repaint is requested only when one moved and the chrome is on screen.
    pub fn sync(snap: Snapshot) {
        let mut c = lock();
        if !c.bar.live() {
            return;
        }
        let moved = c.snap != snap;
        c.snap = snap;
        unsafe {
            place(&mut c);
            if c.visible {
                // Every tick while the chrome is up: mpv reparents and recreates
                // its output window as files change, and a sibling we failed to
                // raise is a control bar under the video.
                raise(&c);
                if moved {
                    InvalidateRect(c.bar.get(), std::ptr::null(), 0);
                }
                if c.list_open {
                    InvalidateRect(c.panel.get(), std::ptr::null(), 0);
                }
                if !snap.playing {
                    // Paused or finished: the chrome stays put, it does not walk
                    // away while the user is deciding what to do next.
                    cancel_fade(&mut c);
                }
            }
        }
    }

    pub fn set_playlist(rows: &[String], current: Option<usize>) {
        let mut c = lock();
        if c.rows.as_slice() != rows {
            c.rows = rows.to_vec();
        }
        c.current = current;
        if c.panel.live() {
            unsafe {
                let max = max_scroll(&c);
                if c.scroll > max {
                    c.scroll = max;
                }
                if c.visible && c.list_open {
                    InvalidateRect(c.panel.get(), std::ptr::null(), 0);
                }
            }
        }
    }

    /// Draining means clearing: a queued action that outlived the tick would be
    /// applied again and again.
    pub fn take_actions() -> Actions {
        let mut c = lock();
        std::mem::take(&mut c.actions)
    }

    /// Pointer movement over the video. Only the hidden-to-shown transition costs
    /// anything, so a moving mouse is not a repaint source.
    pub fn note_motion() {
        let mut c = lock();
        if !c.bar.live() {
            return;
        }
        unsafe {
            if !c.visible {
                show(&mut c);
            } else if c.snap.playing && !c.fading_out {
                restart_idle(&c);
            }
        }
    }

    /// A click on the picture means "get out of the way".
    pub fn note_video_click() {
        let mut c = lock();
        if c.visible && c.snap.playing && !c.dragging {
            unsafe { start_fade_out(&mut c) };
        }
    }

    /// After a resize: re-derive both windows' bounds and raise them back above
    /// the video.
    pub fn relayout() {
        let mut c = lock();
        if c.bar.live() {
            unsafe {
                place(&mut c);
            }
            if c.visible {
                unsafe { raise(&c) };
            }
        }
    }

    pub fn toggle_playlist() {
        let mut c = lock();
        c.list_open = !c.list_open;
        c.actions.toggle_playlist = true;
        if !c.panel.live() {
            return;
        }
        unsafe {
            if c.list_open && !c.visible {
                show(&mut c);
            }
            InvalidateRect(c.panel.get(), std::ptr::null(), 0);
        }
    }

    // --- Window creation ---------------------------------------------------
    const CLASS_NAME: &str = "LarrezChrome";

    unsafe fn create_chrome(c: &mut Chrome) -> Result<(), String> {
        if !c.registered {
            register_class()?;
            c.registered = true;
        }
        let parent = c.parent.get();
        if !c.bar.live() {
            c.bar = Wrap(child_window("Larrez transport", parent));
        }
        if !c.panel.live() {
            c.panel = Wrap(child_window("Larrez playlist", parent));
        }
        if !c.bar.live() || !c.panel.live() {
            return Err("could not create the overlay windows".into());
        }
        for w in [c.bar, c.panel] {
            // The colour key makes the corners of the card see-through and
            // click-through; the alpha is what makes the card read as glass.
            SetLayeredWindowAttributes(w.get(), KEY, ALPHA_SOLID, LWA_COLORKEY | LWA_ALPHA);
            ShowWindow(w.get(), SW_HIDE);
        }
        c.visible = false;
        c.alpha = 0;
        place(c);
        Ok(())
    }

    unsafe fn register_class() -> Result<(), String> {
        let name = wide(CLASS_NAME);
        let cls = WndClassEx {
            size: std::mem::size_of::<WndClassEx>() as u32,
            style: CS_DROPSHADOW,
            wnd_proc: Some(wnd_proc),
            class_extra: 0,
            window_extra: 0,
            instance: GetModuleHandleW(std::ptr::null()),
            icon: std::ptr::null_mut(),
            cursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
            // No background brush: we paint every pixel ourselves, and a brush
            // would fill the keyed area with something that is not the key.
            background: std::ptr::null_mut(),
            menu_name: std::ptr::null(),
            class_name: name.as_ptr(),
            small_icon: std::ptr::null_mut(),
        };
        if RegisterClassExW(&cls) != 0 {
            return Ok(());
        }
        if GetLastError() == ERROR_CLASS_ALREADY_EXISTS {
            return Ok(());
        }
        Err("could not register the interface window class".into())
    }

    unsafe fn child_window(title: &str, parent: Hwnd) -> Hwnd {
        let class = wide(CLASS_NAME);
        let caption = wide(title);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOPARENTNOTIFY,
            class.as_ptr(),
            caption.as_ptr(),
            WS_CHILD,
            0,
            0,
            1,
            1,
            parent,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    }

    unsafe fn raise(c: &Chrome) {
        // Panel first, bar last: the bar spans the panel's column at the bottom
        // and should win there.
        for w in [c.panel, c.bar] {
            if w.live() {
                SetWindowPos(
                    w.get(),
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    /// Sizes and positions both windows from the parent's client area. Runs on
    /// the main loop, never from inside the window procedure: SetWindowPos
    /// dispatches WM_SIZE synchronously, and the procedure would re-enter.
    unsafe fn place(c: &mut Chrome) {
        if !c.parent.live() || !c.bar.live() {
            return;
        }
        let mut pr = Rect::default();
        GetClientRect(c.parent.get(), &mut pr);
        let (pw, ph) = (pr.w(), pr.h());
        if pw <= 0 || ph <= 0 {
            return;
        }
        let s = scale_of(c.parent.get());
        let m = MARGIN * s;
        let bar = Rect::new(m, ph - m - BAR_H * s, pw - m, ph - m);
        if bar != c.bar_rect {
            c.bar_rect = bar;
            SetWindowPos(c.bar.get(), HWND_TOP, bar.l, bar.t, bar.w(), bar.h(), SWP_NOACTIVATE);
        }

        let wide = PANEL_W * s;
        let bottom = (bar.t - m).max(m + 80 * s);
        let panel = Rect::new((pw - m - wide).max(m), m, pw - m, bottom);
        if panel != c.panel_rect {
            c.panel_rect = panel;
            SetWindowPos(
                c.panel.get(),
                HWND_TOP,
                panel.l,
                panel.t,
                panel.w(),
                panel.h(),
                SWP_NOACTIVATE,
            );
        }
        let max = max_scroll(c);
        if c.scroll > max {
            c.scroll = max;
        }
    }

    /// Integer DPI scale. `GetDpiForWindow` returns 0 where it does not exist,
    /// which means "assume 96" — the right fallback, since everything here is
    /// laid out in logical pixels anyway.
    unsafe fn scale_of(hwnd: Hwnd) -> c_int {
        let dpi = GetDpiForWindow(hwnd) as c_int;
        if dpi >= 96 {
            dpi / 96
        } else {
            1
        }
    }

    // --- Visibility --------------------------------------------------------
    unsafe fn show(c: &mut Chrome) {
        place(c);
        if !c.visible {
            c.visible = true;
            c.alpha = 0;
            c.fading_out = false;
            ShowWindow(c.bar.get(), SW_SHOWNA);
            if c.list_open {
                ShowWindow(c.panel.get(), SW_SHOWNA);
            }
            raise(c);
        }
        step_alpha(c);
        if c.snap.playing {
            restart_idle(c);
        }
    }

    unsafe fn restart_idle(c: &Chrome) {
        SetTimer(c.bar.get(), TIMER_IDLE, IDLE_MS, std::ptr::null());
    }

    unsafe fn start_fade_out(c: &mut Chrome) {
        c.fading_out = true;
        SetTimer(c.bar.get(), TIMER_FADE, FADE_MS, std::ptr::null());
    }

    unsafe fn cancel_fade(c: &mut Chrome) {
        if !c.fading_out {
            return;
        }
        c.fading_out = false;
        KillTimer(c.bar.get(), TIMER_FADE);
        c.alpha = ALPHA_SOLID;
        apply_alpha(c);
    }

    unsafe fn apply_alpha(c: &Chrome) {
        for w in [c.bar, c.panel] {
            if w.live() {
                SetLayeredWindowAttributes(w.get(), KEY, c.alpha, LWA_COLORKEY | LWA_ALPHA);
            }
        }
    }

    /// Moves the fade one step and hides the windows at the end of it. This is
    /// the only animated thing in the interface and it runs for about 130 ms;
    /// while the chrome is at rest no timer is set at all.
    unsafe fn step_alpha(c: &mut Chrome) {
        let target = if c.fading_out { 0 } else { ALPHA_SOLID as c_int };
        let cur = c.alpha as c_int;
        let next = if cur < target {
            (cur + ALPHA_STEP).min(target)
        } else if cur > target {
            (cur - ALPHA_STEP).max(target)
        } else {
            target
        };
        c.alpha = next as u8;
        apply_alpha(c);

        if next == target {
            KillTimer(c.bar.get(), TIMER_FADE);
            if c.fading_out {
                c.fading_out = false;
                c.visible = false;
                c.hot = None;
                c.hot_row = None;
                for w in [c.bar, c.panel] {
                    if w.live() {
                        ShowWindow(w.get(), SW_HIDE);
                    }
                }
            }
        } else {
            SetTimer(c.bar.get(), TIMER_FADE, FADE_MS, std::ptr::null());
        }
    }

    // --- Layout ------------------------------------------------------------
    unsafe fn bar_layout(hwnd: Hwnd) -> Layout {
        let mut r = Rect::default();
        GetClientRect(hwnd, &mut r);
        let w = r.w();
        let s = scale_of(hwnd);
        let mut l = Layout {
            scale: s,
            ..Default::default()
        };
        if w <= 0 {
            return l;
        }
        l.pill = Rect::new(0, SCRUB_BAND * s, w, BAR_H * s);
        let pad = PAD * s;
        let gap = GAP * s;
        let mid = l.pill.cy();
        let btn = BTN * s;
        let play = BTN_PLAY * s;
        l.compact = w < COMPACT_W * s;

        let mut x = pad;
        for (hit, wd) in [
            (Hit::Prev, btn),
            (Hit::Rewind, btn),
            (Hit::Play, play),
            (Hit::Forward, btn),
            (Hit::Next, btn),
        ] {
            let hh = if hit == Hit::Play { play } else { btn };
            l.hits.push((hit, Rect::new(x, mid - hh / 2, x + wd, mid + hh / 2)));
            x += wd + gap;
        }
        if !l.compact {
            l.time = Rect::new(x, l.pill.t, x + TIME_W * s, l.pill.b);
        }

        let mut rx = w - pad;
        for (hit, wd) in [
            (Hit::Full, btn),
            (Hit::Snap, btn),
            (Hit::List, btn),
            (Hit::Speed, CHIP_W * s),
            (Hit::Volume, VOL_W * s),
            (Hit::Mute, btn),
        ] {
            let rect = Rect::new(rx - wd, mid - btn / 2, rx, mid + btn / 2);
            if hit == Hit::Volume {
                l.volume = rect;
            }
            l.hits.push((hit, rect));
            rx -= wd + gap;
        }

        let band = (SCRUB_BAND * s) / 2;
        l.scrub = Rect::new(pad, band - 6 * s, w - pad, band + 6 * s);
        l
    }

    unsafe fn panel_layout(hwnd: Hwnd) -> Panel {
        let mut r = Rect::default();
        GetClientRect(hwnd, &mut r);
        let (w, h) = (r.w(), r.h());
        let s = scale_of(hwnd);
        let head = Rect::new(0, 0, w, HEAD_H * s);
        let open = Rect::new(w - 88 * s, 9 * s, w - 12 * s, HEAD_H * s - 9 * s);
        let close = Rect::new(w - 124 * s, 9 * s, w - 96 * s, HEAD_H * s - 9 * s);
        Panel {
            head,
            open,
            close,
            body_top: head.b.min(h),
            scale: s,
        }
    }

    unsafe fn max_scroll(c: &Chrome) -> c_int {
        if !c.panel.live() {
            return 0;
        }
        let mut r = Rect::default();
        GetClientRect(c.panel.get(), &mut r);
        let s = scale_of(c.panel.get());
        let need = c.rows.len() as c_int * ROW_H * s + HEAD_H * s;
        (need - r.h()).max(0)
    }

    /// Which row the pointer is over; None for the header.
    unsafe fn row_at(hwnd: Hwnd, c: &Chrome, p: Point) -> Option<usize> {
        let pl = panel_layout(hwnd);
        if p.y < pl.body_top {
            return None;
        }
        let row_h = ROW_H * pl.scale;
        if row_h <= 0 {
            return None;
        }
        let idx = (p.y + c.scroll - pl.body_top) / row_h;
        if idx < 0 {
            return None;
        }
        let n = idx as usize;
        if n < c.rows.len() {
            Some(n)
        } else {
            None
        }
    }

    // --- Window procedure --------------------------------------------------
    unsafe extern "system" fn wnd_proc(
        hwnd: Hwnd,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        match msg {
            // We paint every pixel, so let nothing erase the keyed background.
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut c = lock();
                let mut r = Rect::default();
                GetClientRect(hwnd, &mut r);
                paint(hwnd, &mut c, r.w(), r.h());
                0
            }
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEWHEEL | WM_MOUSELEAVE
            | WM_TIMER => {
                let mut c = lock();
                // A late message after the parent started closing, or while the
                // chrome is faded out, is not worth a repaint.
                if !c.visible || !c.bar.live() {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                let handled = handle_input(&mut c, hwnd, msg, wparam, lparam);
                if handled {
                    0
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn handle_input(
        c: &mut Chrome,
        hwnd: Hwnd,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> bool {
        let is_panel = c.panel.live() && hwnd == c.panel.get();
        let touching = matches!(
            msg,
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEWHEEL
        );

        if touching {
            // The pointer being here holds the chrome open, and arms the leave
            // notification that lets it close again.
            unsafe {
                if c.snap.playing {
                    restart_idle(c);
                }
                if c.fading_out {
                    cancel_fade(c);
                }
                if !c.hover_armed {
                    c.hover_armed = true;
                    let mut evt = TrackMouseEvent {
                        size: std::mem::size_of::<TrackMouseEvent>() as u32,
                        flags: TME_LEAVE,
                        hwnd_track: hwnd,
                        hover_time: HOVER_DEFAULT,
                    };
                    TrackMouseEvent(&mut evt);
                }
            }
        }

        unsafe {
            match msg {
                WM_MOUSEMOVE => {
                    let p = point(lparam);
                    if is_panel {
                        let pl = panel_layout(hwnd);
                        let over = if pl.open.contains(p) {
                            Some(Hit::Open)
                        } else if pl.close.contains(p) {
                            Some(Hit::Close)
                        } else {
                            None
                        };
                        let row = row_at(hwnd, c, p);
                        if over != c.hot || row != c.hot_row {
                            c.hot = over;
                            c.hot_row = row;
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                    } else {
                        let l = bar_layout(hwnd);
                        let over = l.at(p).map(|(h, _)| h);
                        if over != c.hot {
                            c.hot = over;
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                        if c.dragging {
                            if let Some((what, _)) = over {
                                drag_update(c, hwnd, what, p);
                            }
                        }
                    }
                    true
                }
                WM_LBUTTONDOWN => {
                    let p = point(lparam);
                    SetCapture(hwnd);
                    if is_panel {
                        let pl = panel_layout(hwnd);
                        c.pressed = if pl.open.contains(p) {
                            Some(Hit::Open)
                        } else if pl.close.contains(p) {
                            Some(Hit::Close)
                        } else {
                            None
                        };
                    } else {
                        let l = bar_layout(hwnd);
                        if let Some((what, _)) = l.at(p) {
                            c.pressed = Some(what);
                            if what.is_slider() {
                                c.dragging = true;
                                drag_update(c, hwnd, what, p);
                            }
                        }
                    }
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    true
                }
                WM_LBUTTONUP => {
                    ReleaseCapture();
                    let p = point(lparam);
                    if is_panel {
                        let pl = panel_layout(hwnd);
                        if pl.open.contains(p) {
                            c.actions.open_files = true;
                        } else if pl.close.contains(p) {
                            c.list_open = false;
                            c.actions.toggle_playlist = true;
                            ShowWindow(c.panel.get(), SW_HIDE);
                        } else if let Some(row) = row_at(hwnd, c, p) {
                            c.actions.play_row = Some(row);
                        }
                    } else {
                        let l = bar_layout(hwnd);
                        match l.at(p).map(|(h, _)| h) {
                            Some(Hit::Play) => c.actions.toggle_pause = true,
                            Some(Hit::Prev) => c.actions.prev = true,
                            Some(Hit::Next) => c.actions.next = true,
                            Some(Hit::Rewind) => {
                                c.actions.seek = Some((c.snap.pos - 10.0).max(0.0))
                            }
                            Some(Hit::Forward) => {
                                let end = c.snap.dur.max(c.snap.pos);
                                c.actions.seek = Some((c.snap.pos + 10.0).min(end));
                            }
                            Some(Hit::Mute) => c.actions.toggle_mute = true,
                            Some(Hit::Speed) => c.actions.cycle_speed = true,
                            Some(Hit::Snap) => c.actions.screenshot = true,
                            Some(Hit::Full) => c.actions.toggle_fullscreen = true,
                            Some(Hit::List) => {
                                c.list_open = !c.list_open;
                                c.actions.toggle_playlist = true;
                                if c.list_open {
                                    ShowWindow(c.panel.get(), SW_SHOWNA);
                                    InvalidateRect(c.panel.get(), std::ptr::null(), 0);
                                } else {
                                    ShowWindow(c.panel.get(), SW_HIDE);
                                }
                            }
                            Some(Hit::Open) | Some(Hit::Close) | Some(Hit::Scrub)
                            | Some(Hit::Volume) => {}
                            None => {}
                        }
                    }
                    c.pressed = None;
                    c.dragging = false;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    true
                }
                WM_MOUSEWHEEL => {
                    let step = ((wparam >> 16) as u32) as u16 as i16 as c_int / 120;
                    if is_panel {
                        let before = c.scroll;
                        let max = max_scroll(c);
                        c.scroll = (c.scroll - step * ROW_H * scale_of(hwnd)).clamp(0, max);
                        if before != c.scroll {
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                    } else {
                        let vol = (c.snap.volume + step as f64 * 2.0).clamp(0.0, 100.0);
                        c.actions.volume = Some(vol);
                        c.snap.volume = vol;
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                    true
                }
                WM_MOUSELEAVE => {
                    c.hover_armed = false;
                    c.hot = None;
                    c.hot_row = None;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                    if c.snap.playing {
                        restart_idle(c);
                    }
                    true
                }
                WM_TIMER => {
                    if wparam == TIMER_IDLE {
                        KillTimer(hwnd, TIMER_IDLE);
                        if c.snap.playing && !c.dragging {
                            start_fade_out(c);
                        }
                    } else if wparam == TIMER_FADE {
                        step_alpha(c);
                    }
                    true
                }
                _ => false,
            }
        }
    }

    /// Applies a scrub or volume drag: the value updates live so the bar feels
    /// immediate, and the request is queued for the main loop, which is the only
    /// thing allowed to talk to mpv.
    unsafe fn drag_update(c: &mut Chrome, hwnd: Hwnd, what: Hit, p: Point) {
        let l = bar_layout(hwnd);
        match what {
            Hit::Scrub if c.snap.dur > 0.0 => {
                let t = l.scrub.frac_x(p.x) * c.snap.dur;
                c.snap.pos = t;
                c.actions.seek = Some(t);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            Hit::Volume => {
                let v = l.volume.frac_x(p.x) * 100.0;
                c.snap.volume = v;
                c.actions.volume = Some(v);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            _ => {}
        }
    }

    // --- Painting ----------------------------------------------------------
    unsafe fn paint(hwnd: Hwnd, c: &mut Chrome, w: c_int, h: c_int) {
        if w <= 0 || h <= 0 {
            return;
        }
        let mut ps = PaintStruct {
            hdc: std::ptr::null_mut(),
            erase: 0,
            update: Rect::default(),
            restore: 0,
            inc_update: 0,
            reserved: [0; 32],
        };
        let dc = BeginPaint(hwnd, &mut ps);
        if dc.is_null() {
            return;
        }
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, w, h);
        let prev = SelectObject(mem, bmp);

        paint_rect(mem, Rect::new(0, 0, w, h), KEY);

        if c.panel.live() && hwnd == c.panel.get() {
            paint_panel(mem, hwnd, c, w, h);
        } else {
            paint_bar(mem, hwnd, c);
        }

        BitBlt(dc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, prev);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }

    unsafe fn paint_bar(dc: Hwnd, hwnd: Hwnd, c: &Chrome) {
        let l = bar_layout(hwnd);
        let s = l.scale;
        card(dc, l.pill, 26 * s);

        // Scrub: track, then what has been demuxed, then what has been played,
        // then the head. The head grows under the pointer — the only reason the
        // bar repaints while the mouse moves.
        let scr = l.scrub;
        let track = Rect::new(scr.l, scr.cy() - 2 * s, scr.r, scr.cy() + 2 * s);
        paint_rect(dc, track, C_TRACK);
        let mut head_x = track.l;
        if c.snap.dur > 0.0 {
            let bx = track.l + (track.w() as f64 * c.snap.buffered.clamp(0.0, 1.0)) as c_int;
            paint_rect(dc, Rect::new(track.l, track.t, bx, track.b), C_BUFFER);
            let px = track.l + (track.w() as f64 * (c.snap.pos / c.snap.dur).clamp(0.0, 1.0)) as c_int;
            paint_rect(dc, Rect::new(track.l, track.t, px, track.b), C_ACCENT);
            head_x = px.max(track.l).min(track.r);
        }
        let grabbing = c.dragging || c.hot == Some(Hit::Scrub) || c.pressed == Some(Hit::Scrub);
        let kr = if grabbing { 8 * s } else { 5 * s };
        circle(dc, head_x, track.cy(), kr, if grabbing { C_ACCENT } else { C_TEXT });

        for (what, r) in &l.hits {
            let hot = c.hot == Some(*what);
            let down = c.pressed == Some(*what);
            match what {
                Hit::Scrub => {}
                Hit::Volume => {
                    let v = Rect::new(r.l, r.cy() - 2 * s, r.r, r.cy() + 2 * s);
                    paint_rect(dc, v, C_TRACK);
                    let f = if c.snap.muted {
                        0.0
                    } else {
                        (c.snap.volume / 100.0).clamp(0.0, 1.0)
                    };
                    let vx = v.l + (v.w() as f64 * f) as c_int;
                    let fill = if c.snap.muted { C_MUTED } else { C_ACCENT };
                    paint_rect(dc, Rect::new(v.l, v.t, vx, v.b), fill);
                    circle(dc, vx, v.cy(), 5 * s, C_TEXT);
                }
                Hit::Speed => {
                    if hot {
                        paint_round(dc, *r, if down { C_PRESS } else { C_HOVER }, 12 * s);
                    }
                    let on = (c.snap.speed - 1.0).abs() > 0.001;
                    text(dc, *r, &format!("{:.2}x", c.snap.speed), 11 * s, FW_NORMAL, if on { C_ACCENT } else { C_MUTED });
                }
                Hit::Play => {
                    paint_round(dc, *r, if down { C_PRESS } else { C_ACCENT }, 14 * s);
                    glyph(dc, *what, r.cx(), r.cy(), if down { C_ACCENT } else { C_ON_ACCENT }, !c.snap.playing, s);
                }
                _ => {
                    if down {
                        paint_round(dc, *r, C_PRESS, 12 * s);
                    } else if hot {
                        paint_round(dc, *r, C_HOVER, 12 * s);
                    }
                    let lit = (*what == Hit::List && c.list_open)
                        || (*what == Hit::Mute && c.snap.muted)
                        || (*what == Hit::Full && c.snap.fullscreen);
                    let color = if lit || hot { C_TEXT } else { C_MUTED };
                    let alt = (*what == Hit::Mute && c.snap.muted) || (*what == Hit::Full && c.snap.fullscreen);
                    glyph(dc, *what, r.cx(), r.cy(), color, alt, s);
                }
            }
        }

        if !l.compact {
            let label = format!("{} / {}", fmt_time(c.snap.pos), fmt_time(c.snap.dur));
            text(dc, l.time, &label, 13 * s, FW_NORMAL, C_TEXT);
        }
    }

    unsafe fn paint_panel(dc: Hwnd, hwnd: Hwnd, c: &Chrome, w: c_int, h: c_int) {
        let pl = panel_layout(hwnd);
        let s = pl.scale;
        card(dc, Rect::new(0, 0, w, h), 18 * s);

        let title = if c.rows.is_empty() {
            "Playlist".to_string()
        } else {
            let cur = c.current.map(|i| i + 1).unwrap_or(0);
            format!("Playlist   {cur} / {}", c.rows.len())
        };
        text(
            dc,
            Rect::new(14 * s, pl.head.t, w - 150 * s, pl.head.b),
            &title,
            12 * s,
            FW_SEMIBOLD,
            C_MUTED,
        );
        paint_round(dc, pl.open, if c.hot == Some(Hit::Open) { C_PRESS } else { C_HOVER }, 10 * s);
        text(dc, pl.open, "Open", 12 * s, FW_NORMAL, C_TEXT);
        if c.hot == Some(Hit::Close) {
            paint_round(dc, pl.close, C_HOVER, 10 * s);
        }
        glyph(dc, Hit::Close, pl.close.cx(), pl.close.cy(), C_MUTED, false, s);

        if c.rows.is_empty() {
            text(
                dc,
                Rect::new(14 * s, pl.body_top + 18 * s, w - 28 * s, pl.body_top + 44 * s),
                "Drop files in, or press Open",
                12 * s,
                FW_NORMAL,
                C_MUTED,
            );
            return;
        }

        let row_h = ROW_H * s;
        let mut y = pl.body_top - c.scroll;
        let mut i = (c.scroll / row_h).max(0);
        while y < h && (i as usize) < c.rows.len() {
            let n = i as usize;
            let r = Rect::new(8 * s, y + 3 * s, w - 8 * s, y + row_h - 3 * s);
            let active = c.current == Some(n);
            if active || c.hot_row == Some(n) {
                paint_round(dc, r, if active { C_PRESS } else { C_HOVER }, 10 * s);
            }
            if active {
                paint_rect(dc, Rect::new(r.l, r.t + 7 * s, r.l + 3 * s, r.b - 7 * s), C_ACCENT);
            }
            row_text(dc, r, &c.rows[n], n, s, if active { C_ACCENT } else { C_TEXT });
            y += row_h;
            i += 1;
        }
    }

    // --- Drawing helpers ---------------------------------------------------
    // Each one restores the DC's previous object before deleting its own, which
    // is what keeps GDI handles from leaking and from being used after deletion.
    unsafe fn paint_rect(dc: Hwnd, r: Rect, color: u32) {
        let b = CreateSolidBrush(color);
        FillRect(dc, &r, b);
        DeleteObject(b);
    }

    unsafe fn paint_round(dc: Hwnd, r: Rect, color: u32, radius: c_int) {
        let b = CreateSolidBrush(color);
        let p = CreatePen(PS_SOLID, 1, color);
        let prev_b = SelectObject(dc, b);
        let prev_p = SelectObject(dc, p);
        RoundRect(dc, r.l, r.t, r.r, r.b, radius, radius);
        SelectObject(dc, prev_b);
        SelectObject(dc, prev_p);
        DeleteObject(b);
        DeleteObject(p);
    }

    /// The card both windows are drawn on. Fill and edge are separate objects
    /// on purpose: a rounded rect in GDI outlines itself with the current pen,
    /// so one colour would give a bar with no visible rim.
    unsafe fn card(dc: Hwnd, r: Rect, radius: c_int) {
        let fill = CreateSolidBrush(C_PILL);
        let edge = CreatePen(PS_SOLID, 1, C_LINE);
        let prev_b = SelectObject(dc, fill);
        let prev_p = SelectObject(dc, edge);
        RoundRect(dc, r.l, r.t, r.r, r.b, radius, radius);
        SelectObject(dc, prev_b);
        SelectObject(dc, prev_p);
        DeleteObject(fill);
        DeleteObject(edge);
    }

    unsafe fn circle(dc: Hwnd, cx: c_int, cy: c_int, rad: c_int, color: u32) {
        let b = CreateSolidBrush(color);
        let prev_b = SelectObject(dc, b);
        let prev_p = SelectObject(dc, GetStockObject(NULL_BRUSH));
        Ellipse(dc, cx - rad, cy - rad, cx + rad, cy + rad);
        SelectObject(dc, prev_b);
        SelectObject(dc, prev_p);
        DeleteObject(b);
    }

    /// Centred text in `r`. `font`/`restore` are folded in so callers cannot
    /// forget them.
    unsafe fn text(dc: Hwnd, r: Rect, s: &str, size: c_int, weight: c_int, color: u32) {
        let face = wide("Segoe UI");
        let font_obj = new_font(&face, size, weight);
        let prev_font = SelectObject(dc, font_obj);
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, color);

        let (ws, n) = text16(s);
        let mut sz = Size::default();
        GetTextExtentPoint32W(dc, ws.as_ptr(), n, &mut sz);
        ExtTextOutW(
            dc,
            r.cx() - sz.cx / 2,
            r.cy() - sz.cy / 2,
            0,
            std::ptr::null(),
            ws.as_ptr(),
            n,
        );
        SelectObject(dc, prev_font);
        DeleteObject(font_obj);
    }

    /// One playlist row: the index, then the name ellipsised into what is left.
    /// Splitting them keeps a long filename from swallowing the number.
    unsafe fn row_text(dc: Hwnd, r: Rect, name: &str, index: usize, s: c_int, color: u32) {
        let face = wide("Segoe UI");
        let font_obj = new_font(&face, 12 * s, FW_NORMAL);
        let prev_font = SelectObject(dc, font_obj);
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, C_MUTED);

        let num = format!("{:>2}", index + 1);
        let (wn, nn) = text16(&num);
        let mut sz = Size::default();
        GetTextExtentPoint32W(dc, wn.as_ptr(), nn, &mut sz);
        let ny = r.cy() - sz.cy / 2;
        let nx = r.l + 10 * s;
        ExtTextOutW(dc, nx, ny, 0, std::ptr::null(), wn.as_ptr(), nn);

        SetTextColor(dc, color);
        let mut body = Rect::new(nx + sz.cx + 8 * s, r.t, r.r - 10 * s, r.b);
        let (wname, nname) = text16(name);
        DrawTextW(
            dc,
            wname.as_ptr(),
            nname,
            &mut body,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(dc, prev_font);
        DeleteObject(font_obj);
    }

    unsafe fn new_font(face: &[u16], size: c_int, weight: c_int) -> Hwnd {
        CreateFontW(
            -size,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            0,
            0,
            CLEARTYPE_QUALITY,
            VARIABLE_PITCH | FF_DONTCARE,
            face.as_ptr(),
        )
    }

    /// The glyphs, drawn rather than fonted: no Segoe MDL2 dependency, and they
    /// scale with the bar instead of snapping to a font's pixel grid. `alt` is
    /// the control's second state — pause bars, muted speaker, restore corners.
    unsafe fn glyph(dc: Hwnd, what: Hit, cx: c_int, cy: c_int, color: u32, alt: bool, s: c_int) {
        let u = s;
        let pen = CreatePen(PS_SOLID, 2 * s, color);
        let brush = CreateSolidBrush(color);
        let prev_pen = SelectObject(dc, pen);
        let prev_brush = SelectObject(dc, brush);
        let outline = GetStockObject(NULL_BRUSH);

        match what {
            Hit::Play => {
                if alt {
                    RoundRect(dc, cx - 6 * u, cy - 7 * u, cx - 1 * u, cy + 7 * u, 2 * u, 2 * u);
                    RoundRect(dc, cx + 1 * u, cy - 7 * u, cx + 6 * u, cy + 7 * u, 2 * u, 2 * u);
                } else {
                    tri(dc, cx - 4 * u, cy - 7 * u, cx + 6 * u, cy, cx - 4 * u, cy + 7 * u);
                }
            }
            Hit::Prev | Hit::Next => {
                let dir = if what == Hit::Prev { -1 } else { 1 };
                let bar = cx - dir * 7 * u;
                Rectangle(dc, bar.min(bar + 2 * u), cy - 7 * u, bar.max(bar + 2 * u), cy + 7 * u);
                tri(dc, cx - dir * 2 * u, cy - 6 * u, cx - dir * 2 * u, cy + 6 * u, cx + dir * 5 * u, cy);
            }
            Hit::Rewind | Hit::Forward => {
                let dir = if what == Hit::Rewind { -1 } else { 1 };
                for k in 0..2 {
                    let back = cx + dir * (6 + k * 6) * u;
                    tri(dc, back, cy - 5 * u, back, cy + 5 * u, back - dir * 4 * u, cy);
                }
            }
            Hit::Mute => {
                let body = [
                    Point { x: cx - 8 * u, y: cy - 3 * u },
                    Point { x: cx - 4 * u, y: cy - 3 * u },
                    Point { x: cx, y: cy - 7 * u },
                    Point { x: cx, y: cy + 7 * u },
                    Point { x: cx - 4 * u, y: cy + 3 * u },
                    Point { x: cx - 8 * u, y: cy + 3 * u },
                ];
                Polygon(dc, body.as_ptr(), body.len() as c_int);
                if alt {
                    SelectObject(dc, outline);
                    line(dc, cx + 3 * u, cy - 4 * u, cx + 8 * u, cy + 4 * u);
                    line(dc, cx + 8 * u, cy - 4 * u, cx + 3 * u, cy + 4 * u);
                    SelectObject(dc, brush);
                } else {
                    // Two bulging strokes stand in for sound waves.
                    for k in 0..2 {
                        let x = cx + (4 + k * 3) * u;
                        let bulge = (3 - k) * u;
                        let pts = [
                            Point { x, y: cy - 5 * u },
                            Point { x: x + bulge, y: cy },
                            Point { x, y: cy + 5 * u },
                        ];
                        Polyline(dc, pts.as_ptr(), 3);
                    }
                }
            }
            Hit::List => {
                SelectObject(dc, outline);
                for k in 0..3 {
                    let y = cy + (k - 1) * 5 * u;
                    let right = if k == 2 { cx + 3 * u } else { cx + 9 * u };
                    line(dc, cx - 9 * u, y, right, y);
                }
                SelectObject(dc, brush);
                tri(dc, cx + 4 * u, cy - 6 * u, cx + 4 * u, cy + 6 * u, cx + 10 * u, cy);
            }
            Hit::Snap => {
                SelectObject(dc, outline);
                RoundRect(dc, cx - 9 * u, cy - 5 * u, cx + 9 * u, cy + 7 * u, 4 * u, 4 * u);
                Rectangle(dc, cx - 4 * u, cy - 8 * u, cx + u, cy - 5 * u);
                Ellipse(dc, cx - 3 * u, cy - u, cx + 3 * u, cy + 4 * u);
                SelectObject(dc, brush);
            }
            Hit::Full => {
                SelectObject(dc, outline);
                let e = 8 * u;
                if alt {
                    Rectangle(dc, cx - e, cy - e + 2 * u, cx + e - 2 * u, cy + e);
                    Rectangle(dc, cx - e + 2 * u, cy - e, cx + e, cy + e - 2 * u);
                } else {
                    for (sx, sy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                        let x = cx + sx * e;
                        let y = cy + sy * e;
                        let pts = [
                            Point { x, y: y - sy * 4 * u },
                            Point { x, y },
                            Point { x: x - sx * 4 * u, y },
                        ];
                        Polyline(dc, pts.as_ptr(), 3);
                    }
                }
                SelectObject(dc, brush);
            }
            Hit::Close => {
                SelectObject(dc, outline);
                let e = 4 * u;
                line(dc, cx - e, cy - e, cx + e, cy + e);
                line(dc, cx + e, cy - e, cx - e, cy + e);
                SelectObject(dc, brush);
            }
            Hit::Open | Hit::Speed | Hit::Scrub | Hit::Volume => {}
        }

        SelectObject(dc, prev_pen);
        SelectObject(dc, prev_brush);
        DeleteObject(pen);
        DeleteObject(brush);
    }

    unsafe fn tri(dc: Hwnd, x0: c_int, y0: c_int, x1: c_int, y1: c_int, x2: c_int, y2: c_int) {
        let pts = [
            Point { x: x0, y: y0 },
            Point { x: x1, y: y1 },
            Point { x: x2, y: y2 },
        ];
        Polygon(dc, pts.as_ptr(), 3);
    }

    unsafe fn line(dc: Hwnd, x0: c_int, y0: c_int, x1: c_int, y1: c_int) {
        let pts = [Point { x: x0, y: y0 }, Point { x: x1, y: y1 }];
        Polyline(dc, pts.as_ptr(), 2);
    }

    fn point(lparam: isize) -> Point {
        Point {
            x: (lparam & 0xFFFF) as u16 as i16 as c_int,
            y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as c_int,
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// A UTF-16 buffer plus its length *without* the terminator. The text calls
    /// take an explicit count, so handing them `buf.len()` would draw the NUL.
    fn text16(s: &str) -> (Vec<u16>, c_int) {
        let buf = OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let len = buf.len() as c_int - 1;
        (buf, len)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn colorrefs_are_bgr() {
            assert_eq!(rgb(0x12, 0x34, 0x56), 0x0056_3412);
        }

        #[test]
        fn rects_are_half_open_and_centred() {
            let r = Rect::new(10, 10, 20, 30);
            assert_eq!((r.w(), r.h()), (10, 20));
            assert_eq!((r.cx(), r.cy()), (15, 20));
            assert!(r.contains(Point { x: 10, y: 10 }));
            assert!(!r.contains(Point { x: 20, y: 10 }));
        }

        #[test]
        fn a_zero_width_rect_is_never_hit() {
            let r = Rect::new(5, 0, 5, 10);
            assert!(!r.contains(Point { x: 5, y: 5 }));
            assert_eq!(r.frac_x(500), 0.0);
        }

        #[test]
        fn drags_clamp_to_the_track() {
            let r = Rect::new(100, 0, 200, 10);
            assert_eq!(r.frac_x(0), 0.0);
            assert!((r.frac_x(150) - 0.5).abs() < 1e-9);
            assert_eq!(r.frac_x(9999), 1.0);
        }

        #[test]
        fn sliders_drag_and_buttons_click() {
            assert!(Hit::Scrub.is_slider());
            assert!(Hit::Volume.is_slider());
            assert!(!Hit::Play.is_slider());
            assert!(!Hit::List.is_slider());
        }

        #[test]
        fn paintstruct_is_the_size_win32_writes_into() {
            // A too-small PaintStruct is how BeginPaint corrupts the stack.
            assert_eq!(std::mem::size_of::<PaintStruct>(), 72);
            assert_eq!(std::mem::size_of::<TrackMouseEvent>(), 24);
            assert_eq!(std::mem::size_of::<WndClassEx>(), 80);
            assert_eq!(std::mem::size_of::<Rect>(), 16);
        }
    }
}

#[cfg(windows)]
pub use platform::{
    attach, dispose, note_motion, note_video_click, relayout, set_playlist, sync, take_actions,
    toggle_playlist,
};

#[cfg(not(windows))]
mod stub {
    use super::{Actions, Hwnd, Snapshot};

    pub fn attach(_hwnd: Hwnd) -> Result<(), String> {
        Ok(())
    }
    pub fn dispose() {}
    pub fn sync(_snap: Snapshot) {}
    pub fn set_playlist(_rows: &[String], _current: Option<usize>) {}
    pub fn take_actions() -> Actions {
        Actions::default()
    }
    pub fn note_motion() {}
    pub fn note_video_click() {}
    pub fn relayout() {}
    pub fn toggle_playlist() {}
}

#[cfg(not(windows))]
pub use stub::{
    attach, dispose, note_motion, note_video_click, relayout, set_playlist, sync, take_actions,
    toggle_playlist,
};
