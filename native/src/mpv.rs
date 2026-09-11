//! Minimal, hand-rolled FFI binding to libmpv (the engine behind mpv).
//!
//! We bind only the client-API surface the player actually needs, which keeps
//! the dependency tree at zero crates for the decode path. libmpv is loaded at
//! runtime from `mpv-2.dll` (Windows) / `libmpv.so.2` (Linux), so the player
//! starts even if the user has a differently-versioned mpv on PATH.

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::ptr;

#[repr(C)]
pub struct MpvHandle {
    _private: [u8; 0],
}

/// mpv_format values we use.
const MPV_FORMAT_STRING: c_int = 1;
const MPV_FORMAT_FLAG: c_int = 3;
const MPV_FORMAT_INT64: c_int = 4;
const MPV_FORMAT_DOUBLE: c_int = 5;

/// mpv_event_id values we care about.
pub const MPV_EVENT_NONE: c_int = 0;
pub const MPV_EVENT_SHUTDOWN: c_int = 1;
pub const MPV_EVENT_FILE_LOADED: c_int = 8;
pub const MPV_EVENT_END_FILE: c_int = 7;
pub const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;

#[repr(C)]
pub struct MpvEvent {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEventProperty {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

type FnCreate = unsafe extern "C" fn() -> *mut MpvHandle;
type FnInitialize = unsafe extern "C" fn(*mut MpvHandle) -> c_int;
type FnTerminate = unsafe extern "C" fn(*mut MpvHandle);
type FnCommand = unsafe extern "C" fn(*mut MpvHandle, *mut *const c_char) -> c_int;
type FnSetOption = unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int;
type FnSetProperty = unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int;
type FnGetProperty = unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int;
type FnObserveProperty = unsafe extern "C" fn(*mut MpvHandle, u64, *const c_char, c_int) -> c_int;
type FnWaitEvent = unsafe extern "C" fn(*mut MpvHandle, c_double) -> *mut MpvEvent;
type FnErrorString = unsafe extern "C" fn(c_int) -> *const c_char;

/// Resolved libmpv entry points.
struct Api {
    create: FnCreate,
    initialize: FnInitialize,
    terminate: FnTerminate,
    command: FnCommand,
    set_option: FnSetOption,
    set_property: FnSetProperty,
    get_property: FnGetProperty,
    observe_property: FnObserveProperty,
    wait_event: FnWaitEvent,
    error_string: FnErrorString,
    _lib: DynLib,
}

/// Thin cross-platform dynamic-library loader (no `libloading` dependency).
struct DynLib(*mut c_void);
unsafe impl Send for DynLib {}
unsafe impl Sync for DynLib {}

#[cfg(windows)]
mod sys {
    use super::*;
    extern "system" {
        fn LoadLibraryA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
    }
    pub unsafe fn open(name: &CStr) -> *mut c_void {
        LoadLibraryA(name.as_ptr())
    }
    pub unsafe fn sym(lib: *mut c_void, name: &CStr) -> *mut c_void {
        GetProcAddress(lib, name.as_ptr())
    }
    pub unsafe fn close(lib: *mut c_void) {
        FreeLibrary(lib);
    }
    /// DLL names to try, in order of preference.
    pub const CANDIDATES: &[&str] = &["mpv-2.dll", "libmpv-2.dll", "mpv-1.dll"];
}

#[cfg(not(windows))]
mod sys {
    use super::*;
    extern "C" {
        fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
    }
    const RTLD_NOW: c_int = 2;
    pub unsafe fn open(name: &CStr) -> *mut c_void {
        dlopen(name.as_ptr(), RTLD_NOW)
    }
    pub unsafe fn sym(lib: *mut c_void, name: &CStr) -> *mut c_void {
        dlsym(lib, name.as_ptr())
    }
    pub unsafe fn close(lib: *mut c_void) {
        dlclose(lib);
    }
    pub const CANDIDATES: &[&str] = &["libmpv.so.2", "libmpv.so.1", "libmpv.so"];
}

impl Drop for DynLib {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { sys::close(self.0) };
        }
    }
}

fn load_api() -> Result<Api, String> {
    let mut handle = ptr::null_mut();
    for name in sys::CANDIDATES {
        let c = CString::new(*name).unwrap();
        let h = unsafe { sys::open(&c) };
        if !h.is_null() {
            handle = h;
            break;
        }
    }
    if handle.is_null() {
        return Err(format!(
            "Could not load libmpv. Tried: {}.\n\
             On Windows, place mpv-2.dll next to larrez-player.exe \
             (get it from https://sourceforge.net/projects/mpv-player-windows/files/libmpv/).",
            sys::CANDIDATES.join(", ")
        ));
    }

    macro_rules! sym {
        ($name:literal) => {{
            let c = CString::new($name).unwrap();
            let p = unsafe { sys::sym(handle, &c) };
            if p.is_null() {
                return Err(format!("libmpv is missing symbol `{}` — version too old?", $name));
            }
            unsafe { std::mem::transmute(p) }
        }};
    }

    Ok(Api {
        create: sym!("mpv_create"),
        initialize: sym!("mpv_initialize"),
        terminate: sym!("mpv_terminate_destroy"),
        command: sym!("mpv_command"),
        set_option: sym!("mpv_set_option_string"),
        set_property: sym!("mpv_set_property"),
        get_property: sym!("mpv_get_property"),
        observe_property: sym!("mpv_observe_property"),
        wait_event: sym!("mpv_wait_event"),
        error_string: sym!("mpv_error_string"),
        _lib: DynLib(handle),
    })
}

/// Safe-ish wrapper around an initialized mpv core.
pub struct Mpv {
    api: Api,
    ctx: *mut MpvHandle,
}

impl Mpv {
    /// Create an mpv core rendering into an existing native window.
    ///
    /// `wid` is the platform window id (HWND on Windows, X11 Window on Linux).
    /// mpv creates its own child surface inside it and drives its own render
    /// loop, which is what keeps our process near-idle during playback.
    pub fn new(wid: i64) -> Result<Self, String> {
        let api = load_api()?;
        let ctx = unsafe { (api.create)() };
        if ctx.is_null() {
            return Err("mpv_create failed (out of memory?)".into());
        }

        let mut mpv = Mpv { api, ctx };

        // Embed into our window.
        mpv.set_option("wid", &wid.to_string())?;

        // --- Performance-critical defaults ---
        // Hardware decoding: D3D11VA on Windows, VAAPI/NVDEC elsewhere. This is
        // what keeps CPU in the single digits on 4K HEVC.
        mpv.set_option("hwdec", "auto-safe").ok();
        mpv.set_option("vo", "gpu-next").or_else(|_| mpv.set_option("vo", "gpu"))?;
        mpv.set_option("gpu-api", "d3d11").ok();
        // Don't let mpv spawn its own terminal handlers or config surprises.
        mpv.set_option("terminal", "no").ok();
        mpv.set_option("input-default-bindings", "no").ok();
        mpv.set_option("input-vo-keyboard", "no").ok();
        mpv.set_option("osc", "no").ok();
        // Keep the window alive between files so the UI doesn't flicker.
        mpv.set_option("idle", "yes").ok();
        mpv.set_option("force-window", "yes").ok();
        // Resume support is handled by our own state file, not mpv's.
        mpv.set_option("save-position-on-quit", "no").ok();
        // Modest cache; large enough for network streams, small enough to stay light.
        mpv.set_option("cache", "yes").ok();
        mpv.set_option("demuxer-max-bytes", "64MiB").ok();
        mpv.set_option("demuxer-readahead-secs", "20").ok();

        let rc = unsafe { (mpv.api.initialize)(mpv.ctx) };
        mpv.check(rc, "mpv_initialize")?;

        // Observe the properties that drive our HUD.
        for (id, name, fmt) in [
            (1u64, "time-pos", MPV_FORMAT_DOUBLE),
            (2, "duration", MPV_FORMAT_DOUBLE),
            (3, "pause", MPV_FORMAT_FLAG),
            (4, "volume", MPV_FORMAT_DOUBLE),
            (5, "media-title", MPV_FORMAT_STRING),
            (6, "estimated-vf-fps", MPV_FORMAT_DOUBLE),
            (7, "frame-drop-count", MPV_FORMAT_INT64),
        ] {
            let c = CString::new(name).unwrap();
            unsafe { (mpv.api.observe_property)(mpv.ctx, id, c.as_ptr(), fmt) };
        }

        Ok(mpv)
    }

    fn check(&self, rc: c_int, what: &str) -> Result<(), String> {
        if rc >= 0 {
            return Ok(());
        }
        let msg = unsafe { CStr::from_ptr((self.api.error_string)(rc)) }
            .to_string_lossy()
            .into_owned();
        Err(format!("{what}: {msg}"))
    }

    pub fn set_option(&mut self, key: &str, val: &str) -> Result<(), String> {
        let k = CString::new(key).map_err(|e| e.to_string())?;
        let v = CString::new(val).map_err(|e| e.to_string())?;
        let rc = unsafe { (self.api.set_option)(self.ctx, k.as_ptr(), MPV_FORMAT_STRING, v.as_ptr() as *mut c_void) };
        self.check(rc, &format!("set_option({key})"))
    }

    /// Run an mpv command, e.g. `["loadfile", path, "replace"]`.
    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().filter_map(|a| CString::new(*a).ok()).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        let rc = unsafe { (self.api.command)(self.ctx, ptrs.as_mut_ptr()) };
        self.check(rc, &format!("command({:?})", args.first().unwrap_or(&"")))
    }

    pub fn set_flag(&self, name: &str, on: bool) -> Result<(), String> {
        let c = CString::new(name).map_err(|e| e.to_string())?;
        let mut v: c_int = if on { 1 } else { 0 };
        let rc = unsafe {
            (self.api.set_property)(self.ctx, c.as_ptr(), MPV_FORMAT_FLAG, &mut v as *mut _ as *mut c_void)
        };
        self.check(rc, &format!("set_flag({name})"))
    }

    pub fn set_double(&self, name: &str, val: f64) -> Result<(), String> {
        let c = CString::new(name).map_err(|e| e.to_string())?;
        let mut v = val;
        let rc = unsafe {
            (self.api.set_property)(self.ctx, c.as_ptr(), MPV_FORMAT_DOUBLE, &mut v as *mut _ as *mut c_void)
        };
        self.check(rc, &format!("set_double({name})"))
    }

    pub fn get_double(&self, name: &str) -> Option<f64> {
        let c = CString::new(name).ok()?;
        let mut v: f64 = 0.0;
        let rc = unsafe {
            (self.api.get_property)(self.ctx, c.as_ptr(), MPV_FORMAT_DOUBLE, &mut v as *mut _ as *mut c_void)
        };
        (rc >= 0).then_some(v)
    }

    pub fn get_flag(&self, name: &str) -> Option<bool> {
        let c = CString::new(name).ok()?;
        let mut v: c_int = 0;
        let rc = unsafe {
            (self.api.get_property)(self.ctx, c.as_ptr(), MPV_FORMAT_FLAG, &mut v as *mut _ as *mut c_void)
        };
        (rc >= 0).then_some(v != 0)
    }

    /// Poll the event queue without blocking. Returns the event id.
    pub fn poll_event(&self) -> c_int {
        let ev = unsafe { (self.api.wait_event)(self.ctx, 0.0) };
        if ev.is_null() {
            return MPV_EVENT_NONE;
        }
        unsafe { (*ev).event_id }
    }

    // ---- Convenience wrappers used by the UI layer ----

    pub fn loadfile(&self, path: &str) -> Result<(), String> {
        self.command(&["loadfile", path, "replace"])
    }

    pub fn toggle_pause(&self) {
        let paused = self.get_flag("pause").unwrap_or(false);
        let _ = self.set_flag("pause", !paused);
    }

    pub fn seek_relative(&self, secs: f64) {
        let _ = self.command(&["seek", &secs.to_string(), "relative"]);
    }

    pub fn seek_absolute(&self, secs: f64) {
        let _ = self.command(&["seek", &secs.to_string(), "absolute"]);
    }

    pub fn add_volume(&self, delta: f64) {
        let v = self.get_double("volume").unwrap_or(100.0);
        let _ = self.set_double("volume", (v + delta).clamp(0.0, 130.0));
    }

    pub fn toggle_mute(&self) {
        let m = self.get_flag("mute").unwrap_or(false);
        let _ = self.set_flag("mute", !m);
    }

    pub fn set_speed(&self, rate: f64) {
        let _ = self.set_double("speed", rate.clamp(0.25, 4.0));
    }

    pub fn cycle_audio(&self) {
        let _ = self.command(&["cycle", "aid"]);
    }

    pub fn cycle_subtitles(&self) {
        let _ = self.command(&["cycle", "sid"]);
    }

    pub fn add_subtitle_file(&self, path: &str) {
        let _ = self.command(&["sub-add", path, "select"]);
    }

    pub fn screenshot(&self) {
        let _ = self.command(&["screenshot", "video"]);
    }

    pub fn show_text(&self, msg: &str) {
        let _ = self.command(&["show-text", msg, "1600"]);
    }

    pub fn time_pos(&self) -> f64 {
        self.get_double("time-pos").unwrap_or(0.0)
    }

    pub fn duration(&self) -> f64 {
        self.get_double("duration").unwrap_or(0.0)
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe { (self.api.terminate)(self.ctx) };
            self.ctx = ptr::null_mut();
        }
    }
}
