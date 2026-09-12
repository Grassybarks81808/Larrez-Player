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

The player also writes a log there:

```
%APPDATA%\LarrezPlayer\larrez.log
```

It holds what mpv itself reports — which video output it picked, whether
hardware decoding came up, and any error that stopped the picture. It is the
first thing to look at if something misbehaves, and it is what a bug report
needs.

## Command line

Useful when a file association passes a path, or when you are diagnosing
something:

| Flag | What it does |
| --- | --- |
| `--verbose` | Log everything (mpv's detailed output included) |
| `--safe-mode` | Software decoding, plain GPU path — for machines whose driver or GPU context misbehaves |
| `--version` | Show the version |
| `--help` | Show the shortcuts |

```
larrez-player.exe --verbose "D:\video.mkv"
```

`--verbose` needs a console to be useful for anything other than the log file,
so from Explorer prefer reading `larrez.log`.

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

**The window is blank (white or black) and the video never appears** — the
player starts its window before the video output has painted anything, so what
you are seeing is the absence of a picture rather than a crash. In practice it
means mpv could not start a video output on this machine:

1. Try `larrez-player.exe --safe-mode` — software decoding on the plain GPU
   path, which ignores the hardware-decoding and `gpu-next` paths entirely.
2. Update your graphics driver. Hardware decoding (`D3D11VA`) and the
   `gpu-next` renderer both go through it.
3. Read `%APPDATA%\LarrezPlayer\larrez.log`. It names the video output mpv
   tried and why it gave up, and the player now says so in a dialog as well.

**Video is black but audio plays** — your GPU driver may not support the chosen
hardware decoder for that codec. Run with `--safe-mode` to confirm, then update
your graphics driver.

**The window flashes white for a moment on a slow machine** — it shouldn't: the
window is only shown once mpv owns it, and it is painted black before that. If
you can still see it, the log file and a description of your GPU are welcome in
a [bug report](https://github.com/Grassybarks81808/Larrez-Player/issues).

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
