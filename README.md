# Larrez Player

A lightweight multi-format video player. Plays local and network media without
eating your machine alive.

This repo currently holds the **web prototype** — the UI, playlist engine, and
interaction model that the native Windows build will inherit.

---

## Run it

```bash
npm install
npm run dev      # http://localhost:5173
```

Build a static bundle:

```bash
npm run build    # -> dist/
npm run preview
```

## What it does

**Media**
- Drag & drop files *or* whole folders (directory entries are walked recursively)
- Open files, open folder, or open a direct URL / HLS `.m3u8` / DASH `.mpd` stream
- Per-file format inspection: every track is labelled `native`, `codec-dependent`,
  or `needs native build` **before** you hit play, so nothing fails silently

**Playback**
- Play/pause, ±5s and ±10s skip, frame-accurate scrub bar with hover preview
- Speed 0.25×–4×, volume with mute, audio-track and subtitle-track pickers
- Load external `.srt` or `.vtt` subtitles (SRT is converted to WebVTT in-browser)
- Picture-in-picture, fullscreen, PNG frame snapshot
- Resume-where-you-left-off, persisted per file in `localStorage`

**Playlist**
- Reorder by drag, remove individual items, clear all
- Shuffle, and repeat off / all / one
- Natural sort, so `ep2` lands before `ep10`

**Live stats** — resolution, real presented FPS, dropped frames, and buffer depth
in the sidebar footer.

## Keyboard

| Key | Action | Key | Action |
| --- | --- | --- | --- |
| `Space` / `K` | Play / pause | `M` | Mute |
| `←` / `→` | ∓5 seconds | `F` | Fullscreen |
| `J` / `L` | ∓10 seconds | `I` | Picture-in-picture |
| `↑` / `↓` | Volume | `C` | Snapshot frame |
| `N` / `P` | Next / previous | `S` | Shuffle |
| `O` | Open files | `R` | Cycle repeat |

## Performance notes

The prototype is deliberately built with **no runtime dependencies** — no React,
no player library. The whole app is ~17 kB of JS.

The UI redraws off `requestVideoFrameCallback`, which fires only when the decoder
actually presents a new frame. A paused, hidden, or backgrounded video therefore
costs zero CPU in our code — there is no polling loop anywhere. Controls unmount
from the interaction path when idle. The practical result: essentially all
measurable load during playback belongs to the codec, not to the player.

## Format support: the honest version

Browsers only decode what the host ships codecs for, so the prototype cannot be a
universal player. It classifies each file rather than pretending:

| Container | Web prototype | Native Windows build |
| --- | --- | --- |
| MP4 / M4V (H.264, AV1) | ✅ native | ✅ |
| WebM (VP8/VP9/AV1) | ✅ native | ✅ |
| MOV, MKV, TS | ⚠️ codec-dependent | ✅ |
| HEVC / H.265, DTS, TrueHD | ❌ | ✅ |
| AVI, FLV, WMV, MPEG-PS, M2TS | ❌ | ✅ |

## Roadmap — native Windows build

The next stage is **Rust + libmpv**, packaged as a portable single `.exe`:

- libmpv (the mpv engine) handles demux and decode, so *everything* ffmpeg
  supports plays — no per-container caveats
- D3D11VA hardware decoding on by default; idle RAM in the tens of MB
- This exact UI, rendered natively, with mpv drawing into a child window
- File associations for `.mkv` / `.avi` / `.mp4` arrive in v1.1 alongside an installer

## Layout

```
index.html        markup + control bar
src/main.ts       player logic, playlist, shortcuts, drag & drop
src/formats.ts    codec probing, time/size formatting, SRT→VTT
src/playlist.ts   track model
src/style.css     dark theme
```
