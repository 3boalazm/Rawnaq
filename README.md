# رونق · Rawnaq

Offline restoration of old Quran recitation recordings. Everything runs on your
own machine: no account, no API keys, no server, no internet after install.

## What it does

1. **Diagnose** – noise floor, SNR, clipping, mains hum, digital dropouts.
2. **Repair** (classic DSP, [Cathar](https://crates.io/crates/cathar)) – rebuild
   dropouts, remove clicks, remove 50/60 Hz hum, optionally rebuild clipped peaks.
3. **Detect** – two detectors look at the repaired recording before anything is
   removed. The voice detector combines a speech network
   ([Earshot](https://crates.io/crates/earshot)) with a pitch detector, so it
   finds the reciter without relying on the denoiser's opinion. The noise
   detector measures the steady noise floor as a spectrum.
4. **Denoise** – the measured noise floor is turned down everywhere, including
   under the voice. On top of that,
   [DeepFilterNet 3](https://github.com/Rikorose/DeepFilterNet) (via
   [deep-filter-rt](https://crates.io/crates/deep-filter-rt), on the CPU, model
   embedded in the binary) cleans the gaps and is held back where a voice was
   detected. Both filter the recording; neither generates audio.
5. **Quality gate** – estimates whether the denoiser took level from the voice,
   and lets you listen to exactly what was removed.
6. **Export** – 32-bit float WAV or 24-bit FLAC at the source sample rate.

Each stage is cached. Changing the denoise strength after the first run is
instant, because the strength is a mix of the repaired and the fully denoised
signal and the model is not run again.

## Optional deep engine

The denoise panel offers a second, stronger engine: MossFormer2 (48 kHz, from
[ClearerVoice-Studio](https://github.com/modelscope/ClearerVoice-Studio),
Apache-2.0). It cleans under the voice, which DeepFilterNet is not allowed to
do here. It is a speech model and silences long held notes of a recitation,
so wherever it drops a voice the detector found, that stretch is handed back
to the noise-profile engine.

It needs two files that are not in this repository, in a `models` folder next
to the program, in the app's data folder, or in the project folder:

- `mossformer2_48k.onnx` (229 MB) from
  [TigreGotico/audiosronnx-mossformer2](https://huggingface.co/TigreGotico/audiosronnx-mossformer2)
- `onnxruntime.dll` from ONNX Runtime 1.22 or newer (for example out of the
  `onnxruntime` Python package)

Without them the app runs as before and the "deep" option is disabled.

## Layout

| Path | What |
|---|---|
| `crates/rawnaq-core` | Audio pipeline, no UI. Also builds `rawnaq-cli`. |
| `src-tauri` | Tauri 2 shell: session, stage cache, preview files. |
| `src` | React + TypeScript interface (Arabic, RTL). |

## Run

```
npm install
npm run tauri dev
```

Build an installer with `npm run tauri build`.

Headless, without the UI:

```
cargo run --release -p rawnaq-core --bin rawnaq-cli -- input.mp3 output.wav 0.85 removed.wav
```

### Windows toolchain

Rust's default Windows toolchain needs the Visual Studio "Desktop development
with C++" workload. Without it, use the GNU toolchain for this folder:

```
rustup override set stable-x86_64-pc-windows-gnu
```

Run cargo from PowerShell or cmd, not Git Bash: Git Bash puts its own `link`
ahead of the real linker.

## Long recordings

Three things used to make an hour-long recording look hung, and are fixed:

- Cathar's declicker copies the whole signal for every click it mends, so its
  time grew with the square of the length (10 min took 38 s, an hour would
  have taken most of half an hour). Repair now works on 20-second pieces with
  a little context, side by side.
- One ONNX session spread over all cores stalls on a CPU with fast and slow
  cores. The deep engine now runs several single-thread workers over
  independent four-second segments (`RAWNAQ_ORT="workers,threads"` overrides
  the layout).
- Two channels that are the same signal (old mono tapes saved as stereo) are
  repaired, denoised and mastered once.

The progress bar covers the whole restoration with a time estimate, and the
Cancel button stops it. `RAWNAQ_TRACE=1` prints where the time goes.

## Limits of this version

- CPU only. No GPU path, no hardware detection, no model downloads yet.
- The whole recording is held in memory, several copies of it. A one-hour
  stereo recording peaked at about 12 GB of RAM with the deep engine, so a
  machine with 8 GB will not manage it.
- Time on a laptop CPU (14 threads), measured with the deep engine: about 8 min
  for 43 min of audio and about 13 min for one hour. The fast engine is
  several times quicker. Changing the mastering (tone, level, equaliser,
  voice body, reverb) afterwards redoes only the mastering.
- The quality gate is a heuristic with thresholds that have not been calibrated
  on real recitations. Trust your ears on the "removed" signal first.
- DeepFilterNet was trained on ordinary speech, not on recitation. On old or
  reverberant recordings it treats the reciter as noise, which is why it is
  held back wherever a voice is detected. Long held
  vowels and mosque reverb are where to listen most carefully.
