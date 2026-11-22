**English** · [Русская версия](README.ru.md)

# UPizy

Upscale your videos: **720p / 1080p → 2K (1440p) / 4K (2160p)** — and, optionally,
smooth 60/120 fps.

A native Windows tool written in **Rust**: a single `.exe`, no Python, no Visual Studio,
no dependency installation. Four upscaling engines to choose from:

| Engine | What it does | When to use it |
|---|---|---|
| **AI (Real-ESRGAN)** | A neural network redraws detail: textures, edges, faces, text | Clips, anime, screencasts, short videos |
| **Anime (Anime4K)** | The same networks compiled into GLSL shaders: `Restore_CNN` + `Upscale_CNN_x2` inside the ffmpeg pipeline | Anime and cartoons: an episode in 15–20 minutes |
| **GPU (libplacebo)** | `ewa_lanczossharp` + deband + dither in Vulkan: clean, no invented detail, very fast | Feature films, series, anything long |
| **Turbo (CUDA)** | Decode → `scale_cuda` lanczos → NVENC: everything on the GPU | When you need it *now*: a whole film in minutes |

Also included — **RIFE frame interpolation** (×2 / ×4, **off** by default): 60/120 fps
smoothness without smearing, on its own or together with upscaling.

The interface switches between **Russian and English** with the `RU` / `EN` buttons in the
window header. On the first run the language follows the system language (Russian Windows →
Russian UI, otherwise English).

---

## Quick start

1. Download `UPizy-<version>-win64.7z` and unpack it anywhere.
2. Run `UPizy.exe`.
3. Install **ffmpeg** (see [Requirements](#requirements)) — the app checks the environment
   and tells you in the log what is missing.
4. Need the AI engine? Click **“Download AI”** in the top-right corner (~45 MB, once).
   Need smoothness? Click **“Download RIFE”** in the “Smoothness” section (~410 MB, once).
5. Drop a video into the window → choose the target (2K/4K) → **Start**.

By default the result is saved next to the source file: `film_4K.mkv`.

> The AI/RIFE engines are downloaded by the app into the `bin\` folder next to `UPizy.exe`.
> They are not part of the archive, so the release stays small.

---

## Features

- **Any input format** — decoding is handled by ffmpeg.
- **Upscale to 2K / 4K / ×1.5 / ×2 / ×3 / custom**, keeping the aspect ratio, anamorphic SAR
  and phone rotation.
- **Encoding**: H.265 NVENC (GPU), H.264 NVENC, x265, x264, AV1 (SVT) — pick one or “auto”.
- **10-bit output** (less banding in gradients) for 2K/4K and HDR.
- **Everything is preserved**: audio (copy), subtitles, chapters, metadata, cover art and
  fonts in MKV. For MP4 — compatible tracks are copied, incompatible ones are converted.
- **HDR**: correct tone-mapping to SDR (bt.709) by default; “Keep HDR” retains BT.2020/PQ and
  10 bit (GPU engine only).
- **Interlaced sources**: automatic `bwdif` before upscaling.
- **Smoothness ×2 / ×4** (RIFE, optional): 24 → 48, 30 → 60, 60 → 120 fps and so on.
  Works without upscaling too — “No upscale — smoothness only”.
- **Batch processing**, drag & drop, progress with ETA, log, cancel.
- **Streaming AI pipeline**: frames travel through memory in blocks, the disk is not flooded
  (a 4K film does not need a terabyte of free space).
- **Resumable**: finished files are marked and skipped on the next run.
- **Two UI languages**: Russian and English, switched on the fly.

---

## Speed and quality (measurements)

Hardware: Ryzen 5 5600, RTX 2080 Ti 11 GB, 32 GB RAM. Source: 720p, 30 fps, 90 frames.

| Mode | Time for 90 frames | Roughly |
|---|---|---|
| TURBO · CUDA → 4K | **1.1 s per 96 frames** | ~85 fps, a 23-min episode ≈ **7 min** |
| GPU · libplacebo → 4K | **4.7 s** | ~19 fps, a 1-hour film ≈ 1.3 h |
| AI · animevideo v3 ×3 → 4K | **25 s** | ~3.6 fps, a 1-hour film ≈ 6–7 h |
| AI · animevideo v3 ×2 → 2K | **16 s** | ~5.6 fps |
| AI · x4plus → 2K/4K | **~376 s** | ~0.25 fps — short clips only |

Bottom line: **AI for quality on clips and anime, GPU mode for long films, Turbo when you
cannot wait.** Turbo looks slightly softer than libplacebo (no deband, no CAS), but it is not
blurry: edges and text stay clean, the difference is visible only when comparing frames side
by side.

Typical scaling:

- 720p → 1440p: the AI model runs at exactly ×2, no intermediate resampling.
- 720p → 2160p: ×3, also exact.
- 1080p → 2160p: ×2.
- With x4plus the result is computed at ×4 and downscaled to the target (supersampling).

### How long an episode takes (23 minutes, 1080p)

| Mode | Speed | For 23 minutes |
|---|---|---|
| **Turbo → 4K, no smoothness** | ~117 fps | **≈ 5 minutes** |
| **Anime (Anime4K S) → 4K** | ~31 fps | ≈ 18 minutes |
| **Anime (Anime4K M) → 4K** | ~28 fps | **≈ 20 minutes** |
| **Anime (Anime4K L) → 4K** | ~25 fps | ≈ 22 minutes |
| GPU libplacebo → 4K, no smoothness | ~31 fps | ≈ 17 minutes |
| AI animevideo → 4K, no smoothness | 2.9 fps | ≈ 3 hours |
| GPU → 4K + smoothness ×2 | — | ≈ 2 h 20 min |
| **Anime (Anime4K) → 4K + smoothness ×2** | — | **≈ 2 h 40 min** |
| AI → 4K + smoothness ×2 | 1.1 fps | ≈ 8 h 15 min |

Smoothness is limited by RIFE: on its own it takes about two hours per episode (~5 fps at
1080p). Speed depends on the content: noisy or photographic material makes NVENC and
libplacebo 20–40 % slower than clean anime.

---

## Anime: the Anime4K engine

Select **“Engine” → “Anime · Anime4K”**. These are the same anime networks as in Real-ESRGAN,
but compiled into **GLSL shaders** (mpv `.hook` format) executed by libplacebo inside ffmpeg.

Why it is roughly 10× faster than AI mode:

- no second process and no model reload — everything runs in a single ffmpeg pipeline;
- frames **never go through PNG files** on disk (in AI mode that is ~40 % of the time:
  writing, reading and re-decoding 4K PNGs);
- the shader processes pixels in a single pass instead of block-by-block directory work.

What runs inside (order matters; the shaders are merged into one file):

| Pass | Shader | Purpose |
|---|---|---|
| 1 | `Clamp_Highlights` | tames highlights so bright spots do not get white halos |
| 2 | `Restore_CNN` | removes ringing and edge wobble, restores thin lines |
| 3 | `Upscale_CNN_x2` | edge-aware ×2 upscale; runs twice for 720p → 4K |

**S / M / L** are networks of the same type but different size: S is the fastest and slightly
softer, L is more detailed and ~20 % slower, M is the default balance.

To be honest about limits: Anime4K is built for **drawn content** — lines, flat fills, text.
On photos, skin, noise and fine textures Real-ESRGAN still wins, so AI mode stays the choice
for live-action clips and screencasts. On anime the difference is only visible when comparing
frames up close.

The shaders are **MIT** licensed (© 2019–2021 bloc97) and compiled into the `.exe` — no extra
files are needed next to the program.

---

## Smoothness: frame interpolation

The **“Smoothness · frame interpolation”** setting offers `Off` (default), `×2` or `×4`.
It is independent: you can use it alone (target “No upscale — smoothness only”) or combine it
with any upscaling engine.

The engine is **RIFE (rife-ncnn-vulkan)**, not ffmpeg blending. That matters:

| | RIFE (default) | minterpolate (fallback) |
|---|---|---|
| What it does | A network reconstructs motion and draws the in-between frame | Estimates motion vectors heuristically |
| Anime, lines, text | Clean, no ghosting | “Jelly” artifacts, torn lines |
| Speed (720p) | ~15–20 fps on a 2080 Ti | Many times slower and worse |
| Installation | ~410 MB (“Download RIFE” button) | Built into ffmpeg |

If RIFE is not installed, the app says so in the log, falls back to minterpolate and offers a
download button. Models: **rife-anime** (anime/cartoons, selected automatically for anime
upscaling models) and **rife-v4.6** (live action).

Order of operations: **interpolate at source resolution → upscale**. This way RIFE processes
fewer pixels (faster and more accurate motion), and upscaling then multiplies finished frames.
×4 runs as two ×2 passes.

Modes:

| Setting | Result |
|---|---|
| Smoothness ×2, target 4K | `film_4K_60fps.mkv` — 4K at 60 fps |
| Smoothness ×2, target “smoothness only” | `film_60fps.mkv` — frame size unchanged |
| Smoothness off | as before, `film_4K.mkv` |

Things worth knowing:

- Duplicate frames at internal block boundaries are removed — no stutter or freezes
  (verified by comparing neighbouring frames).
- The final frame count is `2N − 1` (×2) and `4N − 3` (×4): the last frame is not duplicated,
  so the video is about one frame shorter. Audio duration does not change.
- There is no ×3: RIFE doubles, so only ×2 and ×4 are available.
- RIFE and Real-ESRGAN work in 8-bit SDR, so an HDR source is tone-mapped to bt.709 when
  interpolating (same as in AI mode).

---

## CLI

```bat
upizy-cli.exe --help

:: 1080p -> 4K, maximum AI quality
upizy-cli.exe --target 4K --quality max "video.mp4"

:: fast GPU upscale of a whole film to 2K, MKV
upizy-cli.exe --target 2K --engine fast "film.mkv"

:: turbo: 4K in minutes (all on the GPU, slightly softer than libplacebo)
upizy-cli.exe --target 4K --engine turbo "series_ep01.mkv"

:: anime: Anime4K neural shaders — an episode in ~20 minutes
upizy-cli.exe --target 4K --engine anime4k --a4k M "series_ep01.mkv"

:: anime, AI + anime model, custom output folder
upizy-cli.exe --target 4K --model realesrgan-x4plus-anime -o D:\out "series_ep01.mkv"

:: smoothness only: 30 -> 60 fps, resolution unchanged
upizy-cli.exe --target keep --interp 2 "anime.mkv"

:: 4K + 60 fps with the anime interpolation model
upizy-cli.exe --target 4K --interp 2 --interp-model rife-anime "anime.mkv"

:: custom resolution and H.264 for compatibility
upizy-cli.exe --target custom --width 3200 --height 1800 --encoder h264_nvenc --container mp4 "clip.mov"

:: environment diagnostics
upizy-cli.exe --list
```

Main flags: `--target`, `--engine ai|fast|turbo|anime4k`, `--a4k S|M|L`, `--model`,
`--quality max|high|balanced`, `--encoder`, `--container mkv|mp4`, `--interp 1|2|4`,
`--interp-model auto|rife-anime|rife-v4.6`, `--no-interp`, `--sharpen 0..1`, `--denoise`,
`--hdr tonemap|preserve`, `--output`, `--overwrite`, `--keep-work`, `--install-ai`,
`--install-rife`, `--list`.

CLI language: the `UPIZY_LANG=ru|en` environment variable (otherwise the language from
`settings.json` is used).

---

## Requirements

- **Windows 10/11 x64.**
- **ffmpeg + ffprobe in PATH** (a build with NVENC/Vulkan — for example `ffmpeg-full` from
  [gyan.dev](https://www.gyan.dev/ffmpeg/builds/)). You can also put `ffmpeg.exe` and
  `ffprobe.exe` into the `bin\` folder next to the program.
- For the AI engine — a **Vulkan** capable GPU (NVIDIA/AMD/Intel). Downloaded by a button.
- For interpolation — the same Vulkan GPU, RIFE engine (~410 MB) downloaded by a button.
- For H.265 NVENC — NVIDIA Turing or newer.

Environment check:

```bat
upizy-cli.exe --list
```

---

## Building from source

All you need is Rust (GNU toolchain, **Visual Studio is not required**):

```bat
winget install Rustlang.Rustup
rustup default stable-x86_64-pc-windows-gnu

build.bat
```

Output: `target\release\upizy.exe` (GUI) and `upizy-cli.exe` (console);
`build.bat` copies them to the project root as `UPizy.exe` and `UPizy-cli.exe`.

Build arguments used for the release (they keep personal paths out of the binary):

```bat
set RUSTFLAGS=--remap-path-prefix=%USERPROFILE%\UPizy=upizy --remap-path-prefix=%USERPROFILE%=C:\src
cargo build --release
```

---

## Layout

```
src/
  main.rs        GUI binary
  gui.rs         interface (egui/eframe)
  bin/cli.rs     console interface
  lib.rs
  i18n.rs        RU/EN localisation
  config.rs      settings, paths, settings.json
  probe.rs       ffprobe parsing (fps, HDR, SAR, rotation...)
  ffmpeg.rs      ffmpeg lookup, codec/filter cache, hidden windows
  encoders.rs    codec selection, CRF/CQ, track mapping
  ai.rs          Real-ESRGAN (ncnn-vulkan) install/run
  rife.rs        RIFE (frame interpolation) install/run
  anime4k.rs     Anime4K shaders, merged into a single .hook
  pipeline.rs    pipelines: AI (frame streaming), GPU (libplacebo), RIFE
assets/shaders/  Anime4K GLSL shaders (compiled into the exe)
bin/             realesrgan-ncnn-vulkan.exe + rife-ncnn-vulkan.exe + models/ (downloaded)
_work/           temporary frames (removed automatically)
settings.json    last GUI settings
```

### How the AI pipeline works

```
ffmpeg (decode -> PNG into a pipe)
   └─► Rust splits the stream into frames, writes blocks of 16–96 frames
         └─► realesrgan-ncnn-vulkan (whole block, GPU)
               └─► Rust feeds PNGs into the stdin of a second ffmpeg
                     └─► ffmpeg: scale to target -> CAS -> HEVC 10-bit + audio/subtitles
```

A single encoding pass, no segment assembly, bounded disk usage and exact frame count and
duration.

With interpolation one more stage is added:

```
ffmpeg (decode -> PNG)  ─►  RIFE ×2/×4 (in blocks, GPU)  ─►  Real-ESRGAN (optional)  ─►  ffmpeg (encode + audio/subtitles)
```

RIFE works in blocks of 8–120 frames (depending on resolution): one frame is carried into the
next block and duplicates at the boundaries are removed, so pans stay smooth.

---

## Unstable GPU (TDR)

If processing suddenly stops with `GPU is lost`, `CUDA_ERROR_INVALID_VALUE`,
`no suitable device` or `Failed creating Vulkan device`, that is **TDR**: Windows decided the
display driver hung and reset the device. The program is not at fault, but it writes a hint
about this into the log.

What to do, in order:

1. **Reboot the PC.** After a TDR the card often does not come back without a reboot;
   `nvidia-smi` reports `GPU is lost. Reboot the system to recover this GPU`.
2. **Overclock/undervolt.** In MSI Afterburner press Reset, disable “Apply at Windows startup”,
   reboot. Even a “factory” memory overclock on older cards can cause TDRs.
3. **Cap clocks and power** (administrator rights required):

   ```bat
   nvidia-smi -lgc 1400,1500
   nvidia-smi -pl 130
   ```

   If TDRs disappear, the PSU or the card VRM is to blame (dips/ripple under load), not the GPU.
4. **Test the GPU memory**: `memtest_vulkan` (20–30 minutes). Errors mean GDDR6 degradation,
   which only a service centre can fix.
5. **Clean driver reinstall** with DDU in safe mode, then a fresh Studio driver (not Game Ready).
6. **Background GPU hooks**: Riot Vanguard, Wallpaper Engine, Discord/Steam overlays,
   MSI Afterburner — disable them while processing.
7. **Hardware**: two separate 8-pin cables (no daisy chain), clean the heatsink, replace
   thermal pads if the card is older than 5 years.

While the card is unstable, use the **Anime4K S** or **Turbo** engine for long jobs: they put
less and steadier load on the GPU than AI mode with RIFE.

### A confirmed case: silicon degradation

On the test RTX 2080 Ti, long runs (18–25 minutes) consistently failed with TDR at the
~1900 MHz boost clock while drawing 117–142 W. A power limit did not help, and the memory
passed `memtest_vulkan` without errors. The fix was to **lock the core clock into a flat
range** where the card is stable:

```bat
nvidia-smi -lgc 1600,1700
```

After that a 1080p → 4K episode (Anime4K) ran for 25 minutes without a single failure:
1695 MHz in 99.6 % of samples, 56.8 °C peak, the output file intact. No power limit is needed —
under the lock the card draws about 117 W.

To apply the lock automatically at sign-in, create a scheduled task (as administrator):

```bat
schtasks /create /tn "UPizy GPU Clock Lock" /tr "nvidia-smi -lgc 1600,1700" /sc onlogon /rl highest /f
```

Remove the lock: `nvidia-smi -rgc`.

---

## Limitations (honestly)

- **VFR** (variable frame rate) is converted to constant — unnoticeable for screen recordings
  and most films.
- **AI + HDR**: the network works in 8-bit SDR, so an HDR source is tone-mapped to bt.709.
  Need HDR output? Use GPU mode and “Keep HDR”.
- **Dolby Vision** metadata is not carried over (DV metadata is lost).
- **Multichannel audio** is copied as is; in MP4 incompatible codecs (DTS, TrueHD) are
  transcoded to AAC 320k.
- AI upscaling a 4K film takes hours — that is the speed of the neural network, not a program
  limitation.
- **Turbo**: no deband and no CAS (hence slightly softer), 8/10 bit, HDR sources automatically
  use the regular GPU path with tone-mapping. Without NVENC or the `scale_cuda` filter the mode
  falls back to libplacebo and says so in the log.
- **Anime4K** requires the `libplacebo` filter and a working Vulkan GPU; otherwise the engine
  falls back to plain libplacebo (a line in the log tells you). Output is 8/10 bit; on very weak
  GPUs the driver may reset by timeout (TDR), so `S` is the safer choice for old cards.

---

## License and credits

UPizy source code is released under the **MIT** license — see [LICENSE](LICENSE).

The project uses and respects third-party licenses:

- **Real-ESRGAN** (Xintao Wang et al.) — BSD-3-Clause, engine downloaded separately.
- **RIFE / rife-ncnn-vulkan** (nihui) — MIT, engine downloaded separately.
- **Anime4K** (bloc97) — MIT, shaders compiled into the program.
- **ffmpeg** — LGPL/GPL, used as an external executable.
- **egui / eframe**, **serde**, **anyhow**, **rfd** — MIT/Apache-2.0.
