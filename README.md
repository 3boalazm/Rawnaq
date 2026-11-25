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

## Limits of this version

- CPU only. No GPU path, no hardware detection, no model downloads yet.
- The whole recording is held in memory, several copies of it. Expect roughly
  1 GB of RAM per 30 minutes of mono audio.
- The quality gate is a heuristic with thresholds that have not been calibrated
  on real recitations. Trust your ears on the "removed" signal first.
- DeepFilterNet was trained on ordinary speech, not on recitation. On old or
  reverberant recordings it treats the reciter as noise, which is why it is
  held back wherever a voice is detected. Long held
  vowels and mosque reverb are where to listen most carefully.
