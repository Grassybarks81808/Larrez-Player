//! Minimal, hand-rolled FFI binding to libmpv (the engine behind mpv).
//!
//! We bind only the client-API surface the player actually needs, which keeps
//! the dependency tree at zero crates for the decode path. libmpv is loaded at
//! runtime from `mpv-2.dll` (Windows) / `libmpv.so.2` (Linux), so the player
//! starts even if the user has a differently-versioned mpv on PATH.
//!
//! Everything the event loop needs is copied out of mpv's structures inside
//! `poll_event`: mpv only guarantees an event's payload until the next call to
//! `mpv_wait_event`, and a dangling `char *` is exactly the kind of thing that
//! turns a slow frame into a crash.

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::ptr;

#[repr(C)]
pub struct MpvHandle {
    _private: [u8; 0],
}

/// mpv_format values we use.
pub const MPV_FORMAT_STRING: c_int = 1;
pub const MPV_FORMAT_FLAG: c_int = 3;
pub const MPV_FORMAT_INT64: c_int = 4;
pub const MPV_FORMAT_DOUBLE: c_int = 5;

/// mpv_event_id values we act on.
pub const MPV_EVENT_NONE: c_int = 0;
pub const MPV_EVENT_SHUTDOWN: c_int = 1;
pub const MPV_EVENT_LOG_MESSAGE: c_int = 2;
pub const MPV_EVENT_END_FILE: c_int = 7;
pub const MPV_EVENT_FILE_LOADED: c_int = 8;
pub const MPV_EVENT_VIDEO_RECONFIG: c_int = 17;
pub const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;

/// The `mpv_end_file_reason` value that means "this file did not play"
/// (0 = end of file, 2 = stopped, 3 = quit, 5 = redirect).
pub const MPV_END_FILE_REASON_ERROR: c_int = 4;

#[repr(C)]
#[allow(dead_code)] // mirrors the C layout; we only read some of it
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

/// Only the fields that have existed since the first client API are read
/// (`log_level` was added later, so we key off the `level` string instead).
#[repr(C)]
pub struct MpvEventLogMessage {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
}

/// `reason` and `error` are the first two members in every mpv version.
#[repr(C)]
#[allow(dead_code)]
pub struct MpvEventEndFile {
    pub reason: c_int,
    pub error: c_int,
}

type FnCreate = unsafe extern "C" fn() -> *mut MpvHandle;
type FnInitialize = unsafe extern "C" fn(*mut MpvHandle) -> c_int;
type FnTerminate = unsafe extern "C" fn(*mut MpvHandle);
type FnCommand = unsafe extern "C" fn(*mut MpvHandle, *mut *const c_char) -> c_int;
/// `int mpv_set_option_string(mpv_handle *ctx, const char *name, const char *data)`
/// — three arguments, no format. This binding used to describe it as the
/// four-argument `mpv_set_option`, so every option change passed a small
/// integer (`MPV_FORMAT_STRING`, i.e. 1) in the register mpv reads as the value
/// pointer. mpv dereferenced `(char *)1` and the player died with an access
/// violation before it finished starting: the white window that flashed and
/// then vanished. Keep this type and `set_option` in step with the prototype.
type FnSetOptionString = unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int;
type FnSetProperty = unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int;
type FnGetProperty = unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int;
type FnObserveProperty = unsafe extern "C" fn(*mut MpvHandle, u64, *const c_char, c_int) -> c_int;
type FnWaitEvent = unsafe extern "C" fn(*mut MpvHandle, c_double) -> *mut MpvEvent;
type FnErrorString = unsafe extern "C" fn(c_int) -> *const c_char;
type FnRequestLogMessages = unsafe extern "C" fn(*mut MpvHandle, *const c_char) -> c_int;
type FnFree = unsafe extern "C" fn(*mut c_void);

/// Resolved libmpv entry points.
struct Api {
    create: FnCreate,
    initialize: FnInitialize,
    terminate: FnTerminate,
    command: FnCommand,
    set_option_string: FnSetOptionString,
    set_property: FnSetProperty,
    get_property: FnGetProperty,
    observe_property: FnObserveProperty,
    wait_event: FnWaitEvent,
    error_string: FnErrorString,
    request_log_messages: FnRequestLogMessages,
    free: FnFree,
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
            unsafe { sys::close(self.0) }
        }
    }
}

/// Things the player wants to change about how mpv runs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Config {
    /// Ask mpv for verbose logs (they end up in our log file).
    pub verbose: bool,
    /// Give up on the hardware/GPU paths entirely — the escape hatch for
    /// machines where D3D11 or hardware decoding misbehaves.
    pub safe_mode: bool,
}

/// One property change, with the value already copied out of mpv's memory.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    None,
    Flag(bool),
    Double(f64),
    Int(i64),
    Text(String),
}

/// A message from mpv's log.
#[derive(Debug, Clone)]
pub struct LogMessage {
    pub level: String,
    pub prefix: String,
    pub text: String,
}

/// What `poll_event` hands back to the event loop.
#[derive(Debug, Clone)]
pub enum Event {
    /// The queue is empty.
    None,
    Shutdown,
    FileLoaded,
    EndFile {
        reason: c_int,
        error: c_int,
    },
    VideoReconfig,
    PropertyChange {
        id: u64,
        /// The log name of the property, for diagnostics.
        name: String,
        value: Value,
    },
    Log(LogMessage),
    /// An event this player does not care about, with mpv's id for the log.
    Other(c_int),
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

    // Each symbol is spelled together with the type it will be called through,
    // so a signature that drifts from the prototype above is a compile error
    // here rather than an access violation at runtime.
    macro_rules! sym {
        ($name:literal => $ty:ty) => {{
            let c = CString::new($name).unwrap();
            let p = unsafe { sys::sym(handle, &c) };
            if p.is_null() {
                return Err(format!("libmpv is missing symbol `{}` — version too old?", $name));
            }
            unsafe { std::mem::transmute::<*mut c_void, $ty>(p) }
        }};
    }

    Ok(Api {
        create: sym!("mpv_create" => FnCreate),
        initialize: sym!("mpv_initialize" => FnInitialize),
        terminate: sym!("mpv_terminate_destroy" => FnTerminate),
        command: sym!("mpv_command" => FnCommand),
        set_option_string: sym!("mpv_set_option_string" => FnSetOptionString),
        set_property: sym!("mpv_set_property" => FnSetProperty),
        get_property: sym!("mpv_get_property" => FnGetProperty),
        observe_property: sym!("mpv_observe_property" => FnObserveProperty),
        wait_event: sym!("mpv_wait_event" => FnWaitEvent),
        error_string: sym!("mpv_error_string" => FnErrorString),
        request_log_messages: sym!("mpv_request_log_messages" => FnRequestLogMessages),
        free: sym!("mpv_free" => FnFree),
        _lib: DynLib(handle),
    })
}

/// Copy a C string that mpv owns into a `String` of our own.
unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// Set a performance-related option; a refusal is worth a log line, not a
/// failure — the player still works with mpv's own defaults.
fn set_or_log(mpv: &mut Mpv, key: &str, value: &str) {
    if let Err(e) = mpv.set_option(key, value) {
        crate::logging::log("warn", &format!("{key}={value} not accepted: {e}"));
    }
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
    pub fn new(wid: i64, config: Config) -> Result<Self, String> {
        let api = load_api()?;
        let ctx = unsafe { (api.create)() };
        if ctx.is_null() {
            return Err("mpv_create failed (out of memory?)".into());
        }

        let mut mpv = Mpv { api, ctx };

        // Embed into our window. This must happen before mpv_initialize, and
        // it is the one option we insist on: if embedding fails the video ends
        // up in a window of mpv's own and our window stays blank forever.
        //
        // mpv takes the id as an int64 (its docs say to cast an HWND through
        // intptr_t) and treats anything <= 0 as "do not embed", so callers must
        // hand us a positive id — see `window_id()` in main.rs.
        let wid_text = wid.to_string();
        mpv.set_option("wid", &wid_text)
            .map_err(|e| format!("could not embed the video output into our window: {e}"))?;

        // --- Performance-critical defaults ---
        // Hardware decoding: D3D11VA on Windows, VAAPI/NVDEC elsewhere. This is
        // what keeps CPU in the single digits on 4K HEVC.
        if config.safe_mode {
            // Software decoding plus the plain GPU path, for machines whose
            // driver or GPU context chokes on the defaults.
            set_or_log(&mut mpv, "hwdec", "no");
            set_or_log(&mut mpv, "vo", "gpu");
            set_or_log(&mut mpv, "gpu-api", "auto");
        } else {
            set_or_log(&mut mpv, "hwdec", "auto-safe");
            // Give mpv a *list*: if gpu-next cannot start on this machine it
            // falls through to gpu and then to anything else it can find. The
            // trailing comma keeps mpv's own fallbacks in play. Pinning one VO
            // used to leave the window with no picture at all whenever that VO
            // was unavailable — and a window with no picture is a white or
            // black rectangle that users rightly call a crash.
            if let Err(e) = mpv.set_option("vo", "gpu-next,gpu,") {
                crate::logging::log(
                    "warn",
                    &format!("vo=gpu-next,gpu not accepted ({e}); falling back to gpu"),
                );
                set_or_log(&mut mpv, "vo", "gpu,");
            }
        }

        // Don't let mpv spawn its own terminal handlers or config surprises.
        // In verbose mode we *do* want mpv talking, since a user running from a
        // console is usually doing it to read the log.
        mpv.set_option("terminal", if config.verbose { "yes" } else { "no" })
            .ok();
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

        // Route mpv's log through us so it can be written to a file. Without
        // this a failed video output is invisible: the user just gets a window
        // that never paints.
        let level = if config.verbose { "v" } else { "info" };
        if let Err(e) = mpv.request_log_messages(level) {
            // Losing the log is a shame, not a reason to refuse to play.
            crate::logging::log("warn", &format!("could not subscribe to mpv's log: {e}"));
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

    /// Set an option. mpv parses the value itself, exactly as it would a
    /// command line flag, so numbers go in as numbers written out in text.
    pub fn set_option(&mut self, key: &str, val: &str) -> Result<(), String> {
        let k = CString::new(key).map_err(|e| e.to_string())?;
        let v = CString::new(val).map_err(|e| e.to_string())?;
        let rc = unsafe { (self.api.set_option_string)(self.ctx, k.as_ptr(), v.as_ptr()) };
        self.check(rc, &format!("set_option({key})"))
    }

    /// Ask mpv to send us log messages at `level` and above.
    pub fn request_log_messages(&self, level: &str) -> Result<(), String> {
        let l = CString::new(level).map_err(|e| e.to_string())?;
        let rc = unsafe { (self.api.request_log_messages)(self.ctx, l.as_ptr()) };
        self.check(rc, "request_log_messages")
    }

    /// Watch a property; changes arrive as [`Event::PropertyChange`] carrying
    /// the id you pass here.
    pub fn observe(&self, id: u64, name: &str, format: c_int) -> Result<(), String> {
        let n = CString::new(name).map_err(|e| e.to_string())?;
        let rc = unsafe { (self.api.observe_property)(self.ctx, id, n.as_ptr(), format) };
        self.check(rc, &format!("observe({name})"))
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
            (self.api.set_property)(
                self.ctx,
                c.as_ptr(),
                MPV_FORMAT_FLAG,
                &mut v as *mut _ as *mut c_void,
            )
        };
        self.check(rc, &format!("set_flag({name})"))
    }

    pub fn set_double(&self, name: &str, val: f64) -> Result<(), String> {
        let c = CString::new(name).map_err(|e| e.to_string())?;
        let mut v = val;
        let rc = unsafe {
            (self.api.set_property)(
                self.ctx,
                c.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut v as *mut _ as *mut c_void,
            )
        };
        self.check(rc, &format!("set_double({name})"))
    }

    pub fn get_double(&self, name: &str) -> Option<f64> {
        let c = CString::new(name).ok()?;
        let mut v: f64 = 0.0;
        let rc = unsafe {
            (self.api.get_property)(
                self.ctx,
                c.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut v as *mut _ as *mut c_void,
            )
        };
        (rc >= 0).then_some(v)
    }

    pub fn get_flag(&self, name: &str) -> Option<bool> {
        let c = CString::new(name).ok()?;
        let mut v: c_int = 0;
        let rc = unsafe {
            (self.api.get_property)(
                self.ctx,
                c.as_ptr(),
                MPV_FORMAT_FLAG,
                &mut v as *mut _ as *mut c_void,
            )
        };
        (rc >= 0).then_some(v != 0)
    }

    pub fn get_int64(&self, name: &str) -> Option<i64> {
        let c = CString::new(name).ok()?;
        let mut v: i64 = 0;
        let rc = unsafe {
            (self.api.get_property)(
                self.ctx,
                c.as_ptr(),
                MPV_FORMAT_INT64,
                &mut v as *mut _ as *mut c_void,
            )
        };
        (rc >= 0).then_some(v)
    }

    /// Read a string property. mpv allocates the result; `mpv_free` releases it.
    pub fn get_string(&self, name: &str) -> Option<String> {
        let c = CString::new(name).ok()?;
        let mut raw: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            (self.api.get_property)(
                self.ctx,
                c.as_ptr(),
                MPV_FORMAT_STRING,
                &mut raw as *mut _ as *mut c_void,
            )
        };
        if rc < 0 || raw.is_null() {
            return None;
        }
        let s = unsafe { cstr(raw) };
        unsafe { (self.api.free)(raw as *mut c_void) };
        Some(s)
    }

    /// Poll the event queue without blocking, copying anything we keep out of
    /// mpv's memory before returning.
    pub fn poll_event(&self) -> Event {
        let ev = unsafe { (self.api.wait_event)(self.ctx, 0.0) };
        if ev.is_null() {
            return Event::None;
        }
        // SAFETY: the event is owned by mpv and stays valid until the next call.
        let event_id = unsafe { (*ev).event_id };
        let data = unsafe { (*ev).data };
        let userdata = unsafe { (*ev).reply_userdata };

        match event_id {
            MPV_EVENT_NONE => Event::None,
            MPV_EVENT_SHUTDOWN => Event::Shutdown,
            MPV_EVENT_FILE_LOADED => Event::FileLoaded,
            MPV_EVENT_VIDEO_RECONFIG => Event::VideoReconfig,
            MPV_EVENT_END_FILE => {
                if data.is_null() {
                    return Event::EndFile { reason: -1, error: 0 };
                }
                let ef = data as *const MpvEventEndFile;
                Event::EndFile {
                    reason: unsafe { (*ef).reason },
                    error: unsafe { (*ef).error },
                }
            }
            MPV_EVENT_PROPERTY_CHANGE => {
                if data.is_null() {
                    return Event::Other(event_id);
                }
                let p = data as *const MpvEventProperty;
                let name = unsafe { cstr((*p).name) };
                let format = unsafe { (*p).format };
                let payload = unsafe { (*p).data };
                let value = if payload.is_null() {
                    Value::None
                } else {
                    match format {
                        MPV_FORMAT_FLAG => Value::Flag(unsafe { *(payload as *const c_int) } != 0),
                        MPV_FORMAT_INT64 => Value::Int(unsafe { *(payload as *const i64) }),
                        MPV_FORMAT_DOUBLE => Value::Double(unsafe { *(payload as *const c_double) }),
                        MPV_FORMAT_STRING => unsafe { Value::Text(cstr(*(payload as *const *const c_char))) },
                        _ => Value::None,
                    }
                };
                Event::PropertyChange {
                    id: userdata,
                    name,
                    value,
                }
            }
            MPV_EVENT_LOG_MESSAGE => {
                if data.is_null() {
                    return Event::Other(event_id);
                }
                let m = data as *const MpvEventLogMessage;
                Event::Log(LogMessage {
                    level: unsafe { cstr((*m).level) },
                    prefix: unsafe { cstr((*m).prefix) },
                    text: unsafe { cstr((*m).text) },
                })
            }
            other => Event::Other(other),
        }
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

    /// Human-readable text for an mpv error code, for the log.
    pub fn error_text(&self, code: c_int) -> String {
        unsafe { cstr((self.api.error_string)(code)) }
    }

    /// Tear mpv down now, while the host window is still alive: the video
    /// output owns a child window of ours and hooks on our window's thread.
    /// Idempotent, and `Drop` calls it too, so it is always safe to call.
    pub fn shutdown(&mut self) {
        if !self.ctx.is_null() {
            // mpv_terminate_destroy joins mpv's own threads and takes the child
            // window down with it.
            unsafe { (self.api.terminate)(self.ctx) };
            self.ctx = ptr::null_mut();
        }
    }

    /// A one-line description of the video path, for the log file.
    pub fn video_state(&self) -> String {
        let vo = self.get_string("current-vo").unwrap_or_else(|| "?".into());
        let hwdec = self.get_string("hwdec-current").unwrap_or_else(|| "no".into());
        let fmt = self
            .get_string("video-params/pixelformat")
            .unwrap_or_else(|| "?".into());
        let w = self.get_int64("width").unwrap_or(0);
        let h = self.get_int64("height").unwrap_or(0);
        frame_summary(&vo, &hwdec, &fmt, w, h)
    }
}

/// Split out so it can be unit tested without an mpv instance.
fn frame_summary(vo: &str, hwdec: &str, format: &str, width: i64, height: i64) -> String {
    if width > 0 && height > 0 {
        format!("vo={vo} hwdec={hwdec} format={format} {width}x{height}")
    } else {
        format!("vo={vo} hwdec={hwdec} (no video track)")
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_summary_includes_dimensions() {
        assert_eq!(
            frame_summary("gpu-next", "d3d11va", "nv12", 1920, 1080),
            "vo=gpu-next hwdec=d3d11va format=nv12 1920x1080"
        );
    }

    #[test]
    fn video_summary_notices_missing_video() {
        assert!(frame_summary("gpu-next", "no", "?", 0, 0).contains("no video track"));
    }
}
