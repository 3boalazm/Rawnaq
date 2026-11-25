//! MossFormer2 (48 kHz speech enhancement, Alibaba ClearerVoice-Studio,
//! Apache-2.0) run through ONNX Runtime.
//!
//! A far stronger denoiser than DeepFilterNet: it removes noise from *under*
//! the voice. It is also a speech model, and on a recitation it silences whole
//! held notes it does not take for speech. It is therefore never used on its
//! own: [`crate::voice::voice_cut`] finds the stretches where it dropped a
//! detected voice and the pipeline falls back to the profile engine there.
//!
//! The network only predicts a mask from log-mel features. Everything around
//! it (Kaldi-style filter bank, deltas, STFT, overlap-add) is done here and
//! follows ClearerVoice-Studio's `decode_one_audio_mossformer2_se_48k`.
//!
//! Neither the model (229 MB) nor ONNX Runtime ship in the binary. Both are
//! looked for in a `models` folder at run time; without them this engine is
//! simply reported as unavailable.

use crate::resample::resample;
use crate::Progress;
use anyhow::{anyhow, Context, Result};
use cathar::AudioData;
use ort::session::Session;
use ort::value::Tensor;
use realfft::RealFftPlanner;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const MODEL_FILE: &str = "mossformer2_48k.onnx";
pub const RUNTIME_FILE: &str = "onnxruntime.dll";

const SR: u32 = 48_000;
const WIN: usize = 1920;
const HOP: usize = 384;
const BINS: usize = WIN / 2 + 1;
const MELS: usize = 60;
/// The filter bank pads each frame to the next power of two.
const FBANK_FFT: usize = 2048;
/// The model is fed four seconds at a time, three seconds apart; half a
/// second at each edge of a segment is thrown away.
const SEGMENT: usize = SR as usize * 4;
const STRIDE: usize = SR as usize * 3;
const GIVE_UP: usize = (SEGMENT - STRIDE) / 2;
/// The model was trained on samples at 16-bit integer scale.
const INT16: f32 = 32768.0;

/// The folder holding both the model and ONNX Runtime, if there is one.
pub fn find_models(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|d| d.join(MODEL_FILE).is_file() && d.join(RUNTIME_FILE).is_file())
        .cloned()
}

pub struct MossFormer {
    session: Session,
    mel: Vec<Vec<(usize, f32)>>,
    fbank_window: Vec<f32>,
    stft_window: Vec<f32>,
    seed: u64,
}

impl MossFormer {
    /// `dir` must hold [`MODEL_FILE`] and [`RUNTIME_FILE`].
    pub fn load(dir: &Path) -> Result<Self> {
        // ONNX Runtime can be loaded once per process.
        static RUNTIME: OnceLock<std::result::Result<ort::environment::Environment, String>> =
            OnceLock::new();
        let env = RUNTIME
            .get_or_init(|| {
                ort::init_from(dir.join(RUNTIME_FILE))
                    .map_err(|e| e.to_string())
                    .and_then(|builder| builder.build().map_err(|e| e.to_string()))
            })
            .as_ref()
            .map_err(|e| anyhow!("could not load ONNX Runtime: {e}"))?;
        let session = Session::builder(env)
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_file(dir.join(MODEL_FILE))
            .map_err(|e| anyhow!("{e}"))
            .context("could not load the MossFormer2 model")?;
        let hamming = |n: usize| -> Vec<f32> {
            (0..n)
                .map(|i| 0.54 - 0.46 * (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos())
                .collect()
        };
        Ok(Self {
            session,
            mel: mel_bank(),
            fbank_window: hamming(WIN),
            stft_window: hamming(WIN),
            seed: 0x9e37_79b9_7f4a_7c15,
        })
    }

    /// Denoise at full strength. The result has the input's rate and length.
    pub fn denoise(&mut self, audio: &AudioData, progress: Progress) -> Result<AudioData> {
        let sr = audio.sample_rate;
        let n_ch = audio.channels.len().max(1);
        let mut channels = Vec::with_capacity(n_ch);
        for (c, ch) in audio.channels.iter().enumerate() {
            let x = resample(ch, sr, SR);
            let report = |f: f32| progress("denoise", ((c as f32 + f) / n_ch as f32).min(0.99));
            let y = self.channel(&x, &report)?;
            let mut out = resample(&y, SR, sr);
            out.resize(ch.len(), 0.0);
            channels.push(out);
        }
        progress("denoise", 1.0);
        Ok(AudioData { sample_rate: sr, channels })
    }

    fn channel(&mut self, x: &[f32], report: &dyn Fn(f32)) -> Result<Vec<f32>> {
        let n = x.len();
        // Pad so the segments tile the signal exactly.
        let padded_len = if n < SEGMENT + STRIDE {
            SEGMENT + STRIDE
        } else {
            SEGMENT + (n - SEGMENT).div_ceil(STRIDE) * STRIDE
        };
        let mut audio: Vec<f32> = x.iter().map(|s| s * INT16).collect();
        audio.resize(padded_len, 0.0);
        let mut out = vec![0f32; padded_len];
        let mut start = 0;
        while start + SEGMENT <= padded_len {
            let seg = self.segment(&audio[start..start + SEGMENT])?;
            let from = if start == 0 { 0 } else { GIVE_UP };
            out[start + from..start + SEGMENT - GIVE_UP]
                .copy_from_slice(&seg[from..SEGMENT - GIVE_UP]);
            start += STRIDE;
            report(start as f32 / padded_len as f32);
        }
        out.truncate(n);
        out.iter_mut().for_each(|s| *s /= INT16);
        Ok(out)
    }

    /// One four-second segment at integer scale, in and out.
    fn segment(&mut self, seg: &[f32]) -> Result<Vec<f32>> {
        let frames = 1 + (seg.len() - WIN) / HOP;
        let feats = self.features(seg, frames);
        let input = Tensor::from_array(([1usize, frames, MELS * 3], feats)).map_err(|e| anyhow!("{e}"))?;
        let outputs = self.session.run(ort::inputs![input]).map_err(|e| anyhow!("{e}"))?;
        let (shape, mask) = outputs[0].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
        if shape.len() != 3 || shape[1] as usize != frames || shape[2] as usize != BINS {
            return Err(anyhow!("unexpected mask shape {shape:?}"));
        }

        // Mask the spectrum and overlap-add it back.
        let mut planner = RealFftPlanner::<f32>::new();
        let (fwd, inv) = (planner.plan_fft_forward(WIN), planner.plan_fft_inverse(WIN));
        let (mut buf, mut spec) = (fwd.make_input_vec(), fwd.make_output_vec());
        let mut out = vec![0f32; seg.len()];
        let mut envelope = vec![0f32; seg.len()];
        let w = &self.stft_window;
        for t in 0..frames {
            let at = t * HOP;
            buf.iter_mut().zip(&seg[at..]).zip(w).for_each(|((b, s), w)| *b = s * w);
            let _ = fwd.process(&mut buf, &mut spec);
            spec.iter_mut().zip(&mask[t * BINS..]).for_each(|(x, m)| *x *= *m);
            let _ = inv.process(&mut spec, &mut buf);
            for i in 0..WIN {
                out[at + i] += buf[i] / WIN as f32 * w[i];
                envelope[at + i] += w[i] * w[i];
            }
        }
        out.iter_mut().zip(&envelope).for_each(|(o, e)| *o /= e.max(1e-8));
        Ok(out)
    }

    /// Kaldi-style log-mel filter bank with first and second deltas:
    /// `frames × 180`, row by row.
    fn features(&mut self, seg: &[f32], frames: usize) -> Vec<f32> {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FBANK_FFT);
        let (mut buf, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
        let mut fbank = vec![0f32; frames * MELS];
        let mut frame = vec![0f32; WIN];
        for t in 0..frames {
            frame.copy_from_slice(&seg[t * HOP..t * HOP + WIN]);
            // Dither, as in training: without it digital silence is log(0).
            frame.iter_mut().for_each(|s| *s += self.gauss());
            let mean = frame.iter().sum::<f32>() / WIN as f32;
            frame.iter_mut().for_each(|s| *s -= mean);
            for i in (1..WIN).rev() {
                frame[i] -= 0.97 * frame[i - 1];
            }
            frame[0] -= 0.97 * frame[0];
            buf.fill(0.0);
            buf.iter_mut().zip(&frame).zip(&self.fbank_window).for_each(|((b, s), w)| *b = s * w);
            let _ = fft.process(&mut buf, &mut spec);
            for (m, taps) in self.mel.iter().enumerate() {
                let e: f32 = taps.iter().map(|(k, w)| spec[*k].norm_sqr() * w).sum();
                fbank[t * MELS + m] = e.max(f32::EPSILON).ln();
            }
        }
        let d1 = deltas(&fbank, frames);
        let d2 = deltas(&d1, frames);
        let mut feats = Vec::with_capacity(frames * MELS * 3);
        for t in 0..frames {
            for part in [&fbank, &d1, &d2] {
                feats.extend_from_slice(&part[t * MELS..(t + 1) * MELS]);
            }
        }
        feats
    }

    /// A standard normal sample (xorshift + Box–Muller).
    fn gauss(&mut self) -> f32 {
        let mut next = || {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            ((self.seed >> 40) as f32 + 1.0) / (1u64 << 24) as f32
        };
        let (u, v) = (next(), next());
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }
}

/// Triangular filters, evenly spaced on the mel scale from 20 Hz to Nyquist:
/// for each filter, its (bin, weight) taps.
fn mel_bank() -> Vec<Vec<(usize, f32)>> {
    let mel = |hz: f32| 1127.0 * (1.0 + hz / 700.0).ln();
    let (low, high) = (mel(20.0), mel(SR as f32 / 2.0));
    let step = (high - low) / (MELS + 1) as f32;
    let bin_hz = SR as f32 / FBANK_FFT as f32;
    (0..MELS)
        .map(|m| {
            let (left, centre, right) =
                (low + m as f32 * step, low + (m + 1) as f32 * step, low + (m + 2) as f32 * step);
            (0..FBANK_FFT / 2)
                .filter_map(|k| {
                    let x = mel(k as f32 * bin_hz);
                    let w = ((x - left) / (centre - left)).min((right - x) / (right - centre));
                    (w > 0.0).then_some((k, w))
                })
                .collect()
        })
        .collect()
}

/// Regression deltas over ±2 frames, edges repeated: `frames × MELS` in and out.
fn deltas(x: &[f32], frames: usize) -> Vec<f32> {
    let at = |t: isize, m: usize| x[(t.clamp(0, frames as isize - 1) as usize) * MELS + m];
    let mut out = vec![0f32; x.len()];
    for t in 0..frames as isize {
        for m in 0..MELS {
            out[t as usize * MELS + m] =
                ((at(t + 1, m) - at(t - 1, m)) + 2.0 * (at(t + 2, m) - at(t - 2, m))) / 10.0;
        }
    }
    out
}
