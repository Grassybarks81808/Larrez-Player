# Larrez Player — Windows usage

## Install (portable)

1. Download `LarrezPlayer-windows-x64.zip` from the
   [Releases page](https://github.com/Grassybarks81808/Larrez-Player/releases)
   (or from the Actions run artifacts).
2. Unzip it anywhere — a USB stick is fine. Nothing is written to
   `Program Files` and nothing touches the registry.
3. Run `larrez-player.exe`.

The folder must keep `mpv-2.dll` next to the `.exe` — that is the decode engine.

## Playing something

- **Drag & drop** a file or folder onto the window
- Press **O** to open files, **D** to open a folder
- Or associate file types (below) and just double-click a video

## Keyboard

| Key | Action | Key | Action |
| --- | --- | --- | --- |
| `Space` / `K` | Play / pause | `F` | Fullscreen (`Esc` exits) |
| `←` / `→` | ∓5 seconds | `M` | Mute |
| `J` / `L` | ∓10 seconds | `A` | Cycle audio track |
| `↑` / `↓` | Volume | `V` | Cycle subtitle track |
| `N` / `P` | Next / previous | `B` | Load a subtitle file |
| `S` | Shuffle | `C` | Screenshot |
| `R` | Cycle repeat | `I` | Show position info |
| `[` / `]` | Speed down / up | `H` | Help overlay |
| `\` | Reset speed to 1× | `Q` | Quit |

Left-click the video to toggle pause.

## File associations

Not automatic, since the portable build deliberately avoids the registry. To set
them up manually:

1. Right-click any `.mkv` → **Open with** → **Choose another app**
2. Scroll to **Look for another app on this PC**
3. Select your `larrez-player.exe`
4. Tick **Always use this app**

Repeat once per extension you care about (`.mkv`, `.avi`, `.mp4`, `.mov`, `.ts`).

A proper installer that registers these automatically is planned for v1.1.

## Where settings live

Volume, mute, speed, shuffle, repeat, and resume positions are stored in:

```
%APPDATA%\LarrezPlayer\state.json
```

Delete that file to reset the player to defaults.

## Resume behaviour

Positions are remembered per file path, but only when you are **more than 15
seconds in** and **more than 20 seconds from the end** — so quick samples and
finished films don't clutter the list. The file is capped at 300 entries.

## Formats

Anything FFmpeg can demux and decode, which in practice means everything you're
likely to have: MKV, MP4, AVI, MOV, WMV, FLV, TS, M2TS, MPEG, VOB, WebM, OGV,
RM/RMVB, 3GP, DivX, MXF — with H.264, HEVC/H.265, AV1, VP9, MPEG-2, VC-1, and
audio in AAC, AC3, E-AC3, DTS, TrueHD, FLAC, Opus, Vorbis, MP3.

Subtitles: SRT, ASS/SSA (with full styling), VTT, SUB/IDX, and PGS.

## Performance notes

Hardware decoding is on by default (`hwdec=auto-safe`), which on Windows means
**D3D11VA**. The renderer is mpv's `gpu-next` where available, falling back to
`gpu`.

The application process does **no per-frame work**: mpv renders straight into
the window, and our event loop sleeps between 250 ms ticks. Expect idle RAM in
the tens of MB and single-digit CPU on 4K HEVC, because the GPU is doing the
decode.

Tuning knobs, if you want them, go in `%APPDATA%\LarrezPlayer\state.json` — or
edit the defaults in `native/src/mpv.rs` and rebuild.

## Troubleshooting

**"Could not load libmpv"** — `mpv-2.dll` isn't beside the `.exe`. Re-extract the
zip, keeping the files together.

**Video is black but audio plays** — your GPU driver may not support the chosen
hardware decoder for that codec. Rebuild with `hwdec=no` in `native/src/mpv.rs`
to confirm, then update your graphics driver.

**Windows SmartScreen warning** — the binary is unsigned (code-signing
certificates cost money). Click **More info → Run anyway**, or build it yourself
from source.

## Building from source

```powershell
cd native
cargo build --release
```

You need the Rust MSVC toolchain and `mpv-2.dll` alongside the resulting
`target\release\larrez-player.exe`. Grab the DLL from the
[libmpv Windows builds](https://sourceforge.net/projects/mpv-player-windows/files/libmpv/).
