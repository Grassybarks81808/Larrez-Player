//! Larrez Player — native desktop build.
//!
//! Architecture: winit owns a single OS window and the input loop; libmpv is
//! handed that window's raw handle and renders into it directly with hardware
//! decoding. Our process does no per-frame work at all — no GPU context, no
//! render loop, no polling. The event loop sleeps via `ControlFlow::WaitUntil`
//! and wakes ~4×/sec only to service mpv's event queue and persist position.
//!
//! That is the whole "doesn't use much system resources" story: the codec is
//! the only meaningful consumer, and it runs on the GPU.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod mpv;
mod playlist;

use mpv::Mpv;
use playlist::{fmt_time, is_subtitle, Playlist, Repeat, State};
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winit::event::{ElementState, Event, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, WindowBuilder};

const TICK: Duration = Duration::from_millis(250);

fn main() {
    if let Err(e) = run() {
        eprintln!("Larrez Player: {e}");
        // On Windows with the GUI subsystem there's no console, so surface a dialog.
        #[cfg(windows)]
        rfd::MessageDialog::new()
            .set_title("Larrez Player")
            .set_description(&e)
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut state = State::load();

    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let window = WindowBuilder::new()
        .with_title("Larrez Player")
        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0))
        .with_min_inner_size(winit::dpi::LogicalSize::new(480.0, 300.0))
        .build(&event_loop)
        .map_err(|e| e.to_string())?;

    let wid = window_id(&window)?;
    let player = Mpv::new(wid)?;

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
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if !args.is_empty() {
        pl.add_paths(&args);
        if !pl.items.is_empty() {
            open_index(&player, &mut pl, &state, 0);
        }
    } else {
        player.show_text("Larrez Player — press O to open a file, H for help");
    }

    let mut fullscreen = false;
    let mut last_save = Instant::now();

    event_loop
        .run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::WaitUntil(Instant::now() + TICK));

            match event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => {
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
                                if was_empty {
                                    open_index(&player, &mut pl, &state, 0);
                                }
                            }
                        }
                    }

                    WindowEvent::MouseInput { state: st, button, .. } => {
                        if st == ElementState::Pressed && button == MouseButton::Left {
                            player.toggle_pause();
                        }
                    }

                    WindowEvent::KeyboardInput {
                        event: KeyEvent { logical_key, state: ElementState::Pressed, repeat: false, .. },
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

                Event::AboutToWait => {
                    // Drain mpv's event queue.
                    loop {
                        match player.poll_event() {
                            mpv::MPV_EVENT_NONE => break,

                            mpv::MPV_EVENT_FILE_LOADED => {
                                if let Some(e) = pl.current_entry() {
                                    window.set_title(&format!("{} — Larrez Player", e.name));
                                    // Apply the saved position now that duration is known.
                                    if let Some(pos) = state.resume_for(&e.path) {
                                        player.seek_absolute(pos);
                                        player.show_text(&format!("Resumed at {}", fmt_time(pos)));
                                    }
                                }
                            }

                            mpv::MPV_EVENT_END_FILE => {
                                // Advance, honouring shuffle/repeat.
                                if let Some(e) = pl.current_entry() {
                                    let p = e.path.clone();
                                    state.remember(&p, 0.0, 1.0); // clear on completion
                                }
                                match pl.next_index(true) {
                                    Some(n) => open_index(&player, &mut pl, &state, n),
                                    None => player.show_text("End of playlist"),
                                }
                            }

                            mpv::MPV_EVENT_SHUTDOWN => {
                                persist(&player, &mut pl, &mut state);
                                elwt.exit();
                                break;
                            }

                            _ => {}
                        }
                    }

                    // Persist position periodically so a crash doesn't lose it.
                    if last_save.elapsed() > Duration::from_secs(10) {
                        last_save = Instant::now();
                        persist(&player, &mut pl, &mut state);
                    }
                }

                _ => {}
            }
        })
        .map_err(|e| e.to_string())
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
O open file   D open folder   C screenshot   I info   Q quit";

fn adjust_speed(player: &Mpv, state: &mut State, delta: f64) {
    state.speed = (state.speed + delta).clamp(0.25, 4.0);
    player.set_speed(state.speed);
    player.show_text(&format!("Speed {:.2}x", state.speed));
}

fn open_dialog(player: &Mpv, pl: &mut Playlist, state: &State, folder: bool) {
    let picked: Vec<PathBuf> = if folder {
        rfd::FileDialog::new().pick_folder().map(|d| vec![d]).unwrap_or_default()
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
    if was_empty {
        open_index(player, pl, state, 0);
    }
}

fn open_index(player: &Mpv, pl: &mut Playlist, _state: &State, index: usize) {
    let Some(entry) = pl.items.get(index) else { return };
    let path = entry.path.clone();
    pl.current = Some(index);
    if let Err(e) = player.loadfile(&path.to_string_lossy()) {
        player.show_text(&format!("Failed to open: {e}"));
        return;
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
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Extract the platform window id that mpv embeds into.
fn window_id(window: &winit::window::Window) -> Result<i64, String> {
    match window.raw_window_handle() {
        #[cfg(windows)]
        RawWindowHandle::Win32(h) => Ok(h.hwnd as isize as i64),
        #[cfg(not(windows))]
        RawWindowHandle::Xlib(h) => Ok(h.window as i64),
        #[cfg(not(windows))]
        RawWindowHandle::Wayland(_) => Err(
            "Wayland embedding is not supported yet — run with WAYLAND_DISPLAY unset to use X11."
                .to_string(),
        ),
        other => Err(format!("Unsupported window system: {other:?}")),
    }
}
