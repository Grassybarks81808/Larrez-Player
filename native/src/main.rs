//! Larrez Player — native desktop build.
//!
//! Architecture: winit owns a single OS window and the input loop; libmpv is
//! handed that window's raw handle and renders into it directly with hardware
//! decoding. Our process does no per-frame work at all — no GPU context, no
//! render loop, no polling. The event loop sleeps via `ControlFlow::WaitUntil`
//! and wakes ~4×/sec only to service mpv's event queue and persist position.
//!
//! The interface (transport bar, playlist panel) is two layered child windows
//! of ours, painted with GDI and kept above mpv's child - see `ui`. It repaints
//! on pointer input and on the tick below, and only while it is on screen, so
//! this stays true: nothing here runs per frame.
//!
//! That is the whole "doesn't use much system resources" story: the codec is
//! the only meaningful consumer, and it runs on the GPU.
//!
//! Two details around mpv are load-bearing:
//!
//! * **Startup.** The window is created hidden and painted black before it is
//!   shown. A window nothing has painted yet is a white rectangle, and if the
//!   video output fails to start it stays that way — which is what users
//!   report as a crash. Now the window is black from its first frame, and a
//!   video output that never arrives is reported instead of ignored.
//! * **Shutdown.** mpv's video output lives in a child window inside ours and
//!   leaves Win32 hooks on our window's thread, so it is torn down explicitly
//!   (see `Event::LoopExiting`) while our window is still alive.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod logging;
mod mpv;
mod playlist;
mod ui;
#[cfg(windows)]
mod win;

use mpv::{Event as MpvEvent, Mpv, Value};
use playlist::{fmt_time, is_subtitle, Playlist, Repeat, State};
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winit::event::{ElementState, Event as WinitEvent, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowBuilder};

const TICK: Duration = Duration::from_millis(250);
/// How long a file may sit loaded without a picture before we say something.
const VIDEO_OUTPUT_GRACE: Duration = Duration::from_secs(8);

/// Property ids handed to `mpv_observe_property`. They come back in
/// `MpvEvent::PropertyChange`, so we ask for exactly what we act on.
const PROP_VO_CONFIGURED: u64 = 1;

fn main() {
    let args = parse_args();
    logging::init(args.verbose);
    logging::install_panic_hook();

    if let Err(e) = run(&args) {
        logging::log("fatal", &e);
        error_dialog(
            "Larrez Player could not start",
            &format!("{e}\n\nDetails were written to:\n{}", logging::path_display()),
        );
        std::process::exit(1);
    }
}

#[derive(Debug, Default, PartialEq)]
struct Args {
    files: Vec<PathBuf>,
    verbose: bool,
    safe_mode: bool,
}

fn parse_args() -> Args {
    parse_args_from(std::env::args_os().skip(1))
}

/// Split out from `parse_args` so it can be tested without touching the
/// process environment.
fn parse_args_from<I: IntoIterator<Item = OsString>>(argv: I) -> Args {
    let mut args = Args::default();
    for raw in argv {
        let flag = raw.to_string_lossy().into_owned();
        match flag.as_str() {
            "--verbose" | "-v" => args.verbose = true,
            "--safe-mode" => args.safe_mode = true,
            "--version" | "-V" => {
                announce(&format!("Larrez Player {}", env!("CARGO_PKG_VERSION")));
                std::process::exit(0);
            }
            "--help" | "-h" | "/?" => {
                announce(HELP);
                std::process::exit(0);
            }
            _ => args.files.push(PathBuf::from(raw)),
        }
    }
    args
}

/// Show text to the user. A GUI process has no console attached, so printing
/// alone would leave `--help` looking like the app simply did nothing.
fn announce(text: &str) {
    // A GUI process has no console, so a failed write must not be fatal.
    let _ = writeln!(std::io::stdout(), "{text}");
    rfd::MessageDialog::new()
        .set_title("Larrez Player")
        .set_description(text)
        .show();
}

fn error_dialog(title: &str, text: &str) {
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(text)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

fn run(args: &Args) -> Result<(), String> {
    let mut state = State::load();

    // Breadcrumbs. An access violation takes the process down without a
    // whisper otherwise, and the last line in the log is what names the step
    // that did it.
    logging::log("info", "startup: creating the event loop");
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    logging::log("info", "startup: event loop ready");

    // Built hidden on purpose: winit registers its window class without a
    // background brush, so a window that is shown before anything paints it is
    // a white rectangle. We make it black and hand it to mpv first, and only
    // then put it on screen.
    logging::log("info", "startup: creating the window");
    let window = WindowBuilder::new()
        .with_title("Larrez Player")
        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0))
        .with_min_inner_size(winit::dpi::LogicalSize::new(480.0, 300.0))
        .with_visible(false)
        .build(&event_loop)
        .map_err(|e| e.to_string())?;

    logging::log("info", "startup: window created");
    #[cfg(windows)]
    {
        let hwnd = win::hwnd_of(&window)?;
        win::prepare_video_window(hwnd)?;
        logging::log("info", "startup: window prepared for video");
        // The interface is worth having and not worth dying for: if the overlay
        // windows cannot be made, the player still plays, and the log says why.
        if let Err(e) = ui::attach(hwnd) {
            logging::log("warn", &format!("interface unavailable: {e}"));
        }
    }

    let wid = window_id(&window)?;
    logging::log("info", &format!("startup: window id 0x{wid:X}, loading libmpv"));
    let mut player = Mpv::new(
        wid,
        mpv::Config {
            verbose: args.verbose,
            safe_mode: args.safe_mode,
        },
    )?;
    logging::log(
        "info",
        &format!("startup: mpv ready, embedding in window 0x{wid:X}"),
    );
    if args.safe_mode {
        logging::log("info", "safe mode: hardware decoding off, plain GPU path");
    }

    // mpv tells us when a video output is actually up. We use that to report
    // the failure case, rather than leaving a blank window behind.
    if let Err(e) = player.observe(PROP_VO_CONFIGURED, "vo-configured", mpv::MPV_FORMAT_FLAG) {
        logging::log("warn", &format!("could not watch vo-configured: {e}"));
    }

    // Apply persisted preferences.
    let _ = player.set_double("volume", state.volume);
    let _ = player.set_flag("mute", state.muted);
    player.set_speed(state.speed);

    let mut pl = Playlist {
        shuffle: state.shuffle,
        repeat: match state.repeat.as_str() {
            "all" => Repeat::All,
            "one" => Repeat::One,
            _ => Repeat::Off,
        },
        ..Default::default()
    };

    // Files passed on the command line (this is what file associations use).
    if !args.files.is_empty() {
        pl.add_paths(&args.files);
        if !pl.items.is_empty() {
            open_index(&player, &mut pl, &state, 0);
        }
    } else {
        player.show_text("Larrez Player — press O to open a file, H for help");
    }

    // mpv is up; show the window. Anything it has not painted yet is black.
    window.set_visible(true);

    let started = Instant::now();
    let mut alive = true;
    let mut fullscreen = false;
    let mut last_save = Instant::now();
    let mut vo_ready = false;
    let mut vo_reported = false;
    let mut loaded_at: Option<Instant> = None;
    let verbose = args.verbose;

    // Hand the interface what it needs before the window appears, so the first
    // frame the user sees already has its controls in it.
    publish_playlist(&pl);
    ui::sync(snapshot(&player, false));
    ui::note_motion();

    event_loop
        .run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::WaitUntil(Instant::now() + TICK));

            // Once mpv has been shut down its handle is gone; poking it here
            // would be use-after-free in the C library.
            if !alive {
                return;
            }

            match event {
                WinitEvent::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => {
                        logging::log("info", "window closed");
                        persist(&player, &mut pl, &mut state);
                        elwt.exit();
                    }

                    WindowEvent::DroppedFile(path) => {
                        if is_subtitle(&path) {
                            player.add_subtitle_file(&path.to_string_lossy());
                            player.show_text(&format!("Subtitles: {}", name_of(&path)));
                        } else {
                            let was_empty = pl.items.is_empty();
                            let n = pl.add_paths(&[path]);
                            if n > 0 {
                                player.show_text(&format!("Added {n} file(s)"));
                                publish_playlist(&pl);
                                if was_empty {
                                    open_index(&player, &mut pl, &state, 0);
                                }
                            }
                        }
                    }

                    WindowEvent::MouseInput {
                        state: st, button, ..
                    } => {
                        if st == ElementState::Pressed && button == MouseButton::Left {
                            ui::note_video_click();
                            player.toggle_pause();
                        }
                    }

                    WindowEvent::CursorMoved { .. } => ui::note_motion(),

                    // The chrome derives its geometry from the parent, but it has
                    // to be told to look again.
                    WindowEvent::Resized(_) => {
                        ui::relayout();
                    }

                    WindowEvent::KeyboardInput {
                        event:
                            KeyEvent {
                                logical_key,
                                state: ElementState::Pressed,
                                repeat: false,
                                ..
                            },
                        ..
                    } => {
                        handle_key(
                            &logical_key,
                            &player,
                            &mut pl,
                            &mut state,
                            &window,
                            &mut fullscreen,
                            elwt,
                        );
                    }

                    _ => {}
                },

                WinitEvent::AboutToWait => {
                    // Drain mpv's event queue.
                    loop {
                        match player.poll_event() {
                            MpvEvent::None => break,

                            MpvEvent::Log(msg) => {
                                logging::log_mpv(&msg.level, &msg.prefix, &msg.text);
                                // No picture has ever appeared and mpv is
                                // telling us why: stop pretending all is well.
                                if !vo_ready && !vo_reported && video_output_died(&msg.text) {
                                    vo_reported = true;
                                    report_no_video_output(msg.text.trim());
                                }
                            }

                            MpvEvent::PropertyChange { id, value, .. } if id == PROP_VO_CONFIGURED => {
                                match value {
                                    Value::Flag(true) => {
                                        if !vo_ready {
                                            vo_ready = true;
                                            vo_reported = false;
                                            logging::log(
                                                "info",
                                                &format!(
                                                    "video output ready after {} ms ({})",
                                                    started.elapsed().as_millis(),
                                                    player.video_state()
                                                ),
                                            );
                                        }
                                    }
                                    Value::Flag(false) => vo_ready = false,
                                    _ => {}
                                }
                            }

                            // We watch exactly one property, so anything
                            // arriving here is worth a line in a verbose log.
                            MpvEvent::PropertyChange { name, value, .. } if verbose => {
                                logging::log("debug", &format!("{name} is now {value:?}"));
                            }

                            MpvEvent::FileLoaded => {
                                loaded_at = Some(Instant::now());
                                if let Some(e) = pl.current_entry() {
                                    window.set_title(&format!("{} — Larrez Player", e.name));
                                    // Apply the saved position now that duration is known.
                                    if let Some(pos) = state.resume_for(&e.path) {
                                        player.seek_absolute(pos);
                                        player.show_text(&format!("Resumed at {}", fmt_time(pos)));
                                    }
                                }
                            }

                            MpvEvent::EndFile { reason, error } => {
                                let name = pl.current_entry().map(|e| e.name.clone()).unwrap_or_default();
                                if let Some(e) = pl.current_entry() {
                                    let p = e.path.clone();
                                    state.remember(&p, 0.0, 1.0); // clear on completion
                                }

                                if reason == mpv::MPV_END_FILE_REASON_ERROR {
                                    // Don't race through the playlist skipping
                                    // broken files: say which one failed and
                                    // let the user press N.
                                    logging::log(
                                        "error",
                                        &format!(
                                            "{name}: playback failed — {} (reason {reason})",
                                            player.error_text(error)
                                        ),
                                    );
                                    loaded_at = None;
                                    player.show_text(&format!(
                                        "Could not play {name} — press N for the next file"
                                    ));
                                } else {
                                    logging::log("info", &format!("finished {name} (reason {reason})"));
                                    match pl.next_index(true) {
                                        Some(n) => open_index(&player, &mut pl, &state, n),
                                        None => player.show_text("End of playlist"),
                                    }
                                }
                            }

                            MpvEvent::Shutdown => {
                                logging::log("info", "mpv asked us to quit");
                                persist(&player, &mut pl, &mut state);
                                ui::dispose();
                                player.shutdown();
                                alive = false;
                                elwt.exit();
                                break;
                            }

                            MpvEvent::Other(id) if verbose => {
                                logging::log("debug", &format!("unhandled mpv event {id}"));
                            }

                            _ => {}
                        }
                    }

                    // If mpv asked us to quit we have already torn it down;
                    // nothing below may touch its handle again.
                    if !alive {
                        return;
                    }

                    // A file that has been loaded for a while with nothing on
                    // screen means the video output is not coming. Say it once.
                    if !vo_ready
                        && !vo_reported
                        && loaded_at
                            .map(|t| t.elapsed() > VIDEO_OUTPUT_GRACE)
                            .unwrap_or(false)
                    {
                        vo_reported = true;
                        report_no_video_output("no video output after loading a file");
                    }

                    // Persist position periodically so a crash doesn't lose it.
                    if last_save.elapsed() > Duration::from_secs(10) {
                        last_save = Instant::now();
                        persist(&player, &mut pl, &mut state);
                    }

                    // The interface is a client of this tick, not a second loop:
                    // it is told what changed, and hands back what the user asked
                    // for. Nothing it does reaches mpv from a window procedure.
                    ui::sync(snapshot(&player, fullscreen));
                    let actions = ui::take_actions();
                    if actions.any() {
                        apply_actions(
                            actions,
                            &player,
                            &mut pl,
                            &mut state,
                            &window,
                            &mut fullscreen,
                        );
                    }
                }

                WinitEvent::LoopExiting => {
                    ui::dispose();
                    // mpv's video output is a child window of ours and its Win32
                    // hooks sit on this thread, so tear it down while the window
                    // is still alive and before the process starts unwinding.
                    persist(&player, &mut pl, &mut state);
                    player.shutdown();
                    alive = false;
                    logging::log("info", "shutdown complete");
                }

                _ => {}
            }
        })
        .map_err(|e| e.to_string())
}

/// What the bar draws. Read from mpv on the tick rather than pushed from a
/// render loop: a handful of in-process property reads, four times a second,
/// and only while the window is alive.
fn snapshot(player: &Mpv, fullscreen: bool) -> ui::Snapshot {
    let dur = player.duration();
    let pos = player.time_pos();
    let paused = player.get_flag("pause").unwrap_or(true);
    let cached = player.get_double("demuxer-cache-duration").unwrap_or(0.0);
    ui::Snapshot {
        pos,
        dur,
        buffered: if dur > 0.0 {
            ((pos + cached) / dur).clamp(0.0, 1.0)
        } else {
            0.0
        },
        volume: player.get_double("volume").unwrap_or(100.0),
        speed: player.get_double("speed").unwrap_or(1.0),
        playing: !paused && dur > 0.0,
        muted: player.get_flag("mute").unwrap_or(false),
        fullscreen,
    }
}

/// The panel lists what the playlist holds, in the order the user sees it.
fn publish_playlist(pl: &Playlist) {
    let names: Vec<String> = pl.items.iter().map(|e| e.name.clone()).collect();
    ui::set_playlist(&names, pl.current);
}

/// Applies everything the chrome reported since the last tick.
#[allow(clippy::too_many_arguments)]
fn apply_actions(
    actions: ui::Actions,
    player: &Mpv,
    pl: &mut Playlist,
    state: &mut State,
    window: &Window,
    fullscreen: &mut bool,
) {
    if actions.toggle_pause {
        player.toggle_pause();
    }
    if let Some(secs) = actions.seek {
        player.seek_absolute(secs);
    }
    if let Some(volume) = actions.volume {
        state.volume = volume;
        let _ = player.set_double("volume", volume);
    }
    if actions.toggle_mute {
        player.toggle_mute();
        state.muted = player.get_flag("mute").unwrap_or(state.muted);
    }
    if actions.cycle_speed {
        state.speed = next_speed(state.speed);
        player.set_speed(state.speed);
        player.show_text(&format!("Speed {:.2}x", state.speed));
    }
    if actions.toggle_fullscreen {
        *fullscreen = !*fullscreen;
        window.set_fullscreen(fullscreen.then(|| Fullscreen::Borderless(None)));
    }
    if actions.screenshot {
        player.screenshot();
        player.show_text("Screenshot saved");
    }
    if actions.prev {
        if let Some(n) = pl.prev_index() {
            open_index(player, pl, state, n);
        }
    }
    if actions.next {
        if let Some(n) = pl.next_index(false) {
            open_index(player, pl, state, n);
        }
    }
    if let Some(index) = actions.play_row {
        open_index(player, pl, state, index);
    }
    if actions.open_files {
        open_dialog(player, pl, state, false);
    }
    if actions.toggle_playlist {
        // The panel keeps its own open state; this only leaves a trace for when
        // a report asks what the interface was doing at the time.
        logging::log("debug", "playlist panel toggled");
    }
}

/// The speed ladder, wrapping to the start so one control cycles forever.
const SPEEDS: [f64; 8] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];

fn next_speed(cur: f64) -> f64 {
    SPEEDS
        .into_iter()
        .find(|s| *s > cur + 0.001)
        .unwrap_or(SPEEDS[0])
}

#[allow(clippy::too_many_arguments)]
fn handle_key(
    key: &Key,
    player: &Mpv,
    pl: &mut Playlist,
    state: &mut State,
    window: &winit::window::Window,
    fullscreen: &mut bool,
    elwt: &winit::event_loop::EventLoopWindowTarget<()>,
) {
    match key {
        Key::Named(NamedKey::Space) => player.toggle_pause(),
        Key::Named(NamedKey::ArrowLeft) => player.seek_relative(-5.0),
        Key::Named(NamedKey::ArrowRight) => player.seek_relative(5.0),
        Key::Named(NamedKey::ArrowUp) => player.add_volume(5.0),
        Key::Named(NamedKey::ArrowDown) => player.add_volume(-5.0),

        Key::Named(NamedKey::Tab) => ui::toggle_playlist(),

        Key::Named(NamedKey::Escape) => {
            if *fullscreen {
                *fullscreen = false;
                window.set_fullscreen(None);
            }
        }

        Key::Character(c) => match c.to_ascii_lowercase().as_str() {
            "k" => player.toggle_pause(),
            "j" => player.seek_relative(-10.0),
            "l" => player.seek_relative(10.0),
            "m" => player.toggle_mute(),
            "c" => {
                player.screenshot();
                player.show_text("Screenshot saved");
            }
            "a" => player.cycle_audio(),
            "v" => player.cycle_subtitles(),

            "f" => {
                *fullscreen = !*fullscreen;
                window.set_fullscreen(fullscreen.then(|| Fullscreen::Borderless(None)));
            }

            "n" => {
                if let Some(n) = pl.next_index(false) {
                    open_index(player, pl, state, n);
                }
            }
            "p" => {
                // Restart the track if we're well into it, else go back one.
                if player.time_pos() > 3.0 {
                    player.seek_absolute(0.0);
                } else if let Some(n) = pl.prev_index() {
                    open_index(player, pl, state, n);
                }
            }

            "s" => {
                pl.shuffle = !pl.shuffle;
                state.shuffle = pl.shuffle;
                player.show_text(&format!("Shuffle {}", if pl.shuffle { "on" } else { "off" }));
            }
            "r" => {
                pl.repeat = pl.repeat.next();
                state.repeat = pl.repeat.label().into();
                player.show_text(&format!("Repeat: {}", pl.repeat.label()));
            }

            "[" => adjust_speed(player, state, -0.25),
            "]" => adjust_speed(player, state, 0.25),
            "\\" => {
                state.speed = 1.0;
                player.set_speed(1.0);
                player.show_text("Speed 1.00x");
            }

            "o" => open_dialog(player, pl, state, false),
            "d" => open_dialog(player, pl, state, true),
            "b" => {
                if let Some(f) = rfd::FileDialog::new()
                    .add_filter("Subtitles", &["srt", "ass", "ssa", "sub", "vtt"])
                    .pick_file()
                {
                    player.add_subtitle_file(&f.to_string_lossy());
                    player.show_text(&format!("Subtitles: {}", name_of(&f)));
                }
            }

            "i" => {
                let d = player.duration();
                let t = player.time_pos();
                player.show_text(&format!(
                    "{} / {}   [{}/{}]",
                    fmt_time(t),
                    fmt_time(d),
                    pl.current.map(|i| i + 1).unwrap_or(0),
                    pl.items.len()
                ));
            }

            "h" => player.show_text(HELP),

            "q" => {
                persist(player, pl, state);
                elwt.exit();
            }

            _ => {}
        },
        _ => {}
    }
}

const HELP: &str = "Larrez Player\n\
Space/K play-pause   J/L -10s/+10s   arrows seek & volume\n\
F fullscreen   M mute   A audio track   V subtitles   B load subs\n\
N/P next-prev   S shuffle   R repeat   [ ] speed   \\ reset speed\n\
O open file   D open folder   C screenshot   I info   Q quit\n\
Start with --verbose to log everything, --safe-mode to avoid GPU paths";

/// Messages from mpv that mean there is never going to be a picture.
fn video_output_died(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    const FATAL: &[&str] = &[
        "unable to create window",
        "video output failed",
        "could not initialize video output",
        "could not create video output",
        "failed to initialize a video output",
        "error opening/initializing the selected video_out",
    ];
    FATAL.iter().any(|f| m.contains(f))
}

/// A blank window with no explanation is what users (fairly) call a crash, so
/// spell out what happened once and where the details are.
fn report_no_video_output(detail: &str) {
    let detail = detail.trim_end_matches(['\r', '\n']);
    logging::log("error", &format!("no video output: {detail}"));
    let text = format!(
        "Larrez Player could not start its video output.\n\n\
         mpv said: {detail}\n\n\
         Details were written to:\n{}\n\n\
         Worth trying:\n\
         • update your graphics driver\n\
         • start the player with --safe-mode (software decoding, plain GPU path)",
        logging::path_display()
    );
    error_dialog("Larrez Player — no video output", &text);
}

fn adjust_speed(player: &Mpv, state: &mut State, delta: f64) {
    state.speed = (state.speed + delta).clamp(0.25, 4.0);
    player.set_speed(state.speed);
    player.show_text(&format!("Speed {:.2}x", state.speed));
}

fn open_dialog(player: &Mpv, pl: &mut Playlist, state: &State, folder: bool) {
    let picked: Vec<PathBuf> = if folder {
        rfd::FileDialog::new()
            .pick_folder()
            .map(|d| vec![d])
            .unwrap_or_default()
    } else {
        rfd::FileDialog::new()
            .add_filter("Video files", playlist::VIDEO_EXTS)
            .add_filter("All files", &["*"])
            .pick_files()
            .unwrap_or_default()
    };
    if picked.is_empty() {
        return;
    }

    let was_empty = pl.items.is_empty();
    let n = pl.add_paths(&picked);
    if n == 0 {
        player.show_text("No playable video files found");
        return;
    }
    player.show_text(&format!("Added {n} file(s)"));
    publish_playlist(pl);
    if was_empty {
        open_index(player, pl, state, 0);
    }
}

fn open_index(player: &Mpv, pl: &mut Playlist, _state: &State, index: usize) {
    let Some(entry) = pl.items.get(index) else { return };
    publish_playlist(pl);
    let path = entry.path.clone();
    pl.current = Some(index);
    if let Err(e) = player.loadfile(&path.to_string_lossy()) {
        player.show_text(&format!("Failed to open: {e}"));
    }
}

fn persist(player: &Mpv, pl: &mut Playlist, state: &mut State) {
    if let Some(e) = pl.current_entry() {
        let pos = player.time_pos();
        let dur = player.duration();
        if pos > 0.0 {
            let p = e.path.clone();
            state.remember(&p, pos, dur);
        }
    }
    state.volume = player.get_double("volume").unwrap_or(state.volume);
    state.muted = player.get_flag("mute").unwrap_or(state.muted);
    state.shuffle = pl.shuffle;
    state.repeat = pl.repeat.label().into();
    state.save();
}

fn name_of(p: &std::path::Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Extract the platform window id that mpv embeds into.
fn window_id(window: &Window) -> Result<i64, String> {
    match window.raw_window_handle() {
        #[cfg(windows)]
        RawWindowHandle::Win32(h) => {
            // An HWND is a handle, not a number to do arithmetic on: cast
            // through `isize` and bit 31 being set makes it negative, and mpv
            // reads `wid <= 0` as "don't embed" — it then opens a window of its
            // own and ours stays blank forever. Pass it unsigned.
            let id = h.hwnd as usize as i64;
            if id <= 0 {
                return Err(format!("unusable window handle 0x{:X}", h.hwnd as usize));
            }
            Ok(id)
        }
        #[cfg(not(windows))]
        RawWindowHandle::Xlib(h) => Ok(h.window as i64),
        #[cfg(not(windows))]
        RawWindowHandle::Wayland(_) => Err(
            "Wayland embedding is not supported yet — run with WAYLAND_DISPLAY unset to use X11.".to_string(),
        ),
        other => Err(format!("Unsupported window system: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_parsed_and_files_kept() {
        let args = parse_args_from(vec![
            OsString::from("--verbose"),
            OsString::from("ep1.mkv"),
            OsString::from("--safe-mode"),
        ]);
        assert!(args.verbose);
        assert!(args.safe_mode);
        assert_eq!(args.files, vec![PathBuf::from("ep1.mkv")]);
    }

    #[test]
    fn a_file_called_like_a_flag_is_still_a_file() {
        let args = parse_args_from(vec![OsString::from("movie.mkv")]);
        assert!(!args.verbose);
        assert_eq!(args.files, vec![PathBuf::from("movie.mkv")]);
    }

    #[test]
    fn the_speed_ladder_steps_up_and_wraps() {
        assert_eq!(next_speed(1.0), 1.5);
        assert_eq!(next_speed(0.9), 1.0);
        assert_eq!(next_speed(4.0), 0.25);
        // Repeated clicks must land on rungs, so what the bar prints is always
        // a speed mpv was actually given.
        let mut s = 1.0;
        for _ in 0..SPEEDS.len() {
            s = next_speed(s);
            assert!(SPEEDS.contains(&s), "{s} left the ladder");
        }
    }

    #[test]
    fn fatal_video_output_messages_are_recognised() {
        assert!(video_output_died(
            "[vo/gpu-next/win32] unable to create window!\n"
        ));
        assert!(video_output_died("Video output failed.\n"));
        assert!(!video_output_died("[cplayer] Track switched\n"));
        assert!(!video_output_died("[vo/gpu-next] Using display sync\n"));
    }
}
