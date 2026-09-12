# Larrez Player

A lightweight multi-format video player. Plays local and network media without
eating your machine alive.

Two builds live in this repo:

| | Where | What it's for |
| --- | --- | --- |
| **Native Windows player** | [`native/`](native/) | The real thing — Rust + libmpv, portable `.exe`, plays everything |
| **Web prototype** | [`src/`](src/) | The UI/UX reference the native build follows |

---

## Download for Windows

Grab `LarrezPlayer-windows-x64.zip` from
[**Releases**](https://github.com/Grassybarks81808/Larrez-Player/releases),
unzip anywhere, run `larrez-player.exe`. Portable — no installer, no registry
writes. Keep `mpv-2.dll` next to the `.exe`.

Full instructions: [`docs/USAGE.md`](docs/USAGE.md)

## Formats

Anything FFmpeg can demux and decode:

**Containers** — MKV · MP4 · AVI · MOV · WMV · FLV · TS · M2TS · MPEG · VOB ·
WebM · OGV · RM/RMVB · 3GP · DivX · MXF

**Video** — H.264 · HEVC/H.265 · AV1 · VP9 · MPEG-2 · VC-1

**Audio** — AAC · AC3 · E-AC3 · DTS · TrueHD · FLAC · Opus · Vorbis · MP3

**Subtitles** — SRT · ASS/SSA (full styling) · VTT · SUB/IDX · PGS

## Features

- On-screen controls: a floating transport bar over the video, with a scrub
  track you can drag, and a playlist panel — both fade out while you watch and
  come back with the mouse
- `Tab` toggles the playlist; clicking a row plays it
- Drag & drop files or folders; playlist with shuffle and repeat off/all/one
- Resume where you left off, per file
- Audio-track and subtitle-track cycling, external subtitle loading
- Speed 0.25×–4×, fullscreen, screenshots
- Natural sort, so `ep2` plays before `ep10`
- Settings persisted to `%APPDATA%\LarrezPlayer\state.json`
- A log of what the video output did, in `%APPDATA%\LarrezPlayer\larrez.log`
- `--verbose` for the full picture, `--safe-mode` if your GPU paths misbehave

## Keyboard

| Key | Action | Key | Action |
| --- | --- | --- | --- |
| `Space` / `K` | Play / pause | `F` | Fullscreen |
| `←` / `→` | ∓5 seconds | `M` | Mute |
| `J` / `L` | ∓10 seconds | `A` | Cycle audio track |
| `↑` / `↓` | Volume | `V` | Cycle subtitles |
| `N` / `P` | Next / previous | `B` | Load subtitle file |
| `S` / `R` | Shuffle / repeat | `C` | Screenshot |
| `Tab` | Playlist panel | `I` | Position info |
| `[` / `]` / `\` | Speed down / up / reset | `O` / `D` | Open file / folder |
| `I` | Position info | `H` / `Q` | Help / quit |

---

## Why it's light

The player process does **no per-frame work whatsoever** — including the
interface.

libmpv is handed the window's raw `HWND` and renders into it directly, driving
its own presentation on the GPU. We never create a GPU context, never run a
render loop, never poll. The winit event loop sleeps on `ControlFlow::WaitUntil`
and wakes four times a second purely to drain mpv's event queue and persist the
playback position.

Hardware decoding is on by default — **D3D11VA** on Windows via `hwdec=auto-safe`,
with mpv's `gpu-next` renderer where available. The GPU decodes; the CPU mostly
idles. Expect tens of MB of RAM at rest and single-digit CPU on 4K HEVC.

The interface is two layered child windows painted with GDI above mpv's output
window, and it repaints on pointer input and on the tick — never per frame. It
fades itself out on its own timers, so a hidden bar costs the main loop nothing
and a visible one costs four small paints a second.

The binary itself is built with fat LTO, one codegen unit, `panic=abort`, and
stripped symbols.

## Build it yourself

```powershell
cd native
cargo build --release
```

Needs the Rust MSVC toolchain, plus `mpv-2.dll` beside the output binary
([libmpv Windows builds](https://sourceforge.net/projects/mpv-player-windows/files/libmpv/)).

CI (`.github/workflows/build-windows.yml`) builds the `.exe` on a Windows
runner, runs the unit tests, bundles libmpv, **launches the packaged app and
checks that it starts, stays up and shuts down cleanly**, then uploads a
ready-to-run zip on every push. Tagging `v*` publishes it as a release.

---

## Web prototype

```bash
npm install
npm run dev      # http://localhost:5173
```

Vite + vanilla TypeScript, **zero runtime dependencies**, ~17 kB of JS. It has
the same playlist model, shortcuts, resume, and drag & drop, plus a scrub bar
with hover preview and a live FPS/dropped-frame/buffer HUD.

Its UI redraws off `requestVideoFrameCallback`, so a paused or backgrounded
video costs zero CPU in our code.

**Its one real limitation:** browsers only decode what the host ships codecs
for, so AVI, FLV, WMV, MPEG-PS and HEVC cannot play there. Rather than fail
silently, it inspects each file and labels it `native`, `codec-dependent`, or
`needs native build` before you press play. The native build has no such
caveats — that is precisely why it exists.

## Layout

```
native/src/main.rs       window, event loop, input, playlist control
native/src/mpv.rs        hand-rolled libmpv FFI (runtime-loaded, zero crates)
native/src/win.rs        Win32 window setup: black background, WS_CLIPCHILDREN
native/src/logging.rs    log file + panic hook, so failures leave evidence
native/src/playlist.rs   playlist model, natural sort, resume persistence
native/src/ui.rs         the interface: layered overlay windows, GDI painting
src/main.ts              web prototype player logic
src/formats.ts           codec probing, SRT to VTT
docs/USAGE.md            Windows install, shortcuts, troubleshooting
```

## When the picture doesn't come

A window with nothing in it is not a crash — it is mpv saying it could not
start a video output. The player paints that window black (never white), tells
you what mpv reported, and writes the detail to
`%APPDATA%\LarrezPlayer\larrez.log`. `--safe-mode` bypasses the hardware
decoding and `gpu-next` paths if your driver is the problem.

## Licence

MIT — see [LICENSE](LICENSE). libmpv is LGPL-2.1+, loaded dynamically and
shipped unmodified.
