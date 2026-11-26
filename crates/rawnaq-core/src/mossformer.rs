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
//! ## Speed
//!
//! The recording is cut into four-second segments that do not depend on one
//! another, so they are handed out to several workers, each with its own
//! ONNX Runtime session. That keeps every core busy, which one session spread
//! over all cores does not: on a laptop CPU with fast and slow cores one big
//! session spends most of its time waiting for the slowest core. Two channels
//! that are the same recording (an old mono tape saved as stereo) are
//! processed once.
//!
//! Neither the model (229 MB) nor ONNX Runtime ship in the binary. Both are
//! looked for in a `models` folder at run time; without them this engine is
//! simply reported as unavailable.

use crate::resample::resample;
use crate::{Cancel, Progress};
use anyhow::{anyhow, Context, Result};
use cathar::AudioData;
use ort::session::Session;
use ort::value::Tensor;
use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub const MODEL_FILE: &str = "mossformer2_48k.onnx";
pub const RUNTIME_FILE: &str = "onnxruntime.dll";

const SR: u32 = 48_000;
const WIN: usize = 1920;
const HOP: usize = 384;
const BINS: usize = WIN / 2 + 1;
const MELS: usize = 60;
/// The filter bank pads each frame to the next power of two.
const FBANK_FFT: usize = 2048;
/// The model is fed four seconds at a time, three seconds apart, so each
/// segment shares its first and last second with a neighbour. A quarter of a
/// second at each edge is thrown away (the model has no context there) and the
/// half second in between is a crossfade from one segment to the next.
///
/// The reference implementation cuts straight from one segment to the next.
/// The two never agree exactly, so that leaves a small step in the waveform
/// every three seconds: a tick, and with many of them, a crackle.
const SEGMENT: usize = SR as usize * 4;
const STRIDE: usize = SR as usize * 3;
const GIVE_UP: usize = SR as usize / 4;

/// How much of segment `k` is used at its sample `i`: the two segments that
/// cover a sample always add up to one.
fn weight(k: usize, i: usize) -> f32 {
    let overlap = SEGMENT - STRIDE;
    let ramp = (overlap - 2 * GIVE_UP) as f32;
    let rising = if k == 0 || i >= overlap - GIVE_UP {
        1.0
    } else if i < GIVE_UP {
        0.0
    } else {
        (i - GIVE_UP) as f32 / ramp
    };
    let falling = if i < STRIDE + GIVE_UP {
        1.0
    } else if i >= SEGMENT - GIVE_UP {
        0.0
    } else {
        1.0 - (i - STRIDE - GIVE_UP) as f32 / ramp
    };
    rising * falling
}
/// The model was trained on samples at 16-bit integer scale.
const INT16: f32 = 32768.0;

/// The folder holding both the model and ONNX Runtime, if there is one.
pub fn find_models(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|d| d.join(MODEL_FILE).is_file() && d.join(RUNTIME_FILE).is_file())
        .cloned()
}

/// Workers and threads per worker. `RAWNAQ_ORT="workers,threads"` overrides it,
/// for measuring on a new machine.
fn plan() -> (usize, usize) {
    if let Some((w, t)) = std::env::var("RAWNAQ_ORT").ok().and_then(|v| {
        let (w, t) = v.split_once(',')?;
        Some((w.trim().parse::<usize>().ok()?, t.trim().parse::<usize>().ok()?))
    }) {
        return (w.max(1), t.max(1));
    }
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    // One thread per worker: measured on a 14-thread laptop CPU, six workers of
    // one thread each ran about twice as fast as one session over all cores
    // (which stalls waiting for its slowest core), and about as fast as fourteen
    // while holding less than half the memory: each worker has its own copy of
    // the model (about 230 MB).
    ((cores / 2).clamp(1, 6), 1)
}

/// Everything about the signal processing around the network that does not
/// change from one segment to the next.
struct Dsp {
    mel: Vec<Vec<(usize, f32)>>,
    hamming: Vec<f32>,
    fbank_fft: Arc<dyn RealToComplex<f32>>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
}

pub struct MossFormer {
    sessions: Vec<Session>,
    dsp: Dsp,
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
        let (workers, threads) = plan();
        let model = dir.join(MODEL_FILE);
        // Loading a session takes seconds: load them side by side.
        let sessions: Vec<Result<Session>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    let model = &model;
                    s.spawn(move || -> Result<Session> {
                        let builder = Session::builder(env).map_err(|e| anyhow!("{e}"))?;
                        let mut builder = builder.with_intra_threads(threads).map_err(|e| anyhow!("{e}"))?;
                        builder
                            .commit_from_file(model)
                            .map_err(|e| anyhow!("{e}"))
                            .context("could not load the MossFormer2 model")
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Err(anyhow!("model loading panicked"))))
                .collect()
        });
        let sessions = sessions.into_iter().collect::<Result<Vec<_>>>()?;

        let mut planner = RealFftPlanner::<f32>::new();
        let hamming =
            (0..WIN).map(|i| 0.54 - 0.46 * (std::f32::consts::TAU * i as f32 / (WIN - 1) as f32).cos()).collect();
        let dsp = Dsp {
            mel: mel_bank(),
            hamming,
            fbank_fft: planner.plan_fft_forward(FBANK_FFT),
            fwd: planner.plan_fft_forward(WIN),
            inv: planner.plan_fft_inverse(WIN),
        };
        Ok(Self { sessions, dsp })
    }

    /// Denoise at full strength. The result has the input's rate and length.
    pub fn denoise(&mut self, audio: &AudioData, progress: Progress, cancel: Cancel) -> Result<AudioData> {
        let sr = audio.sample_rate;
        let sources = crate::mirror_sources(audio);
        let unique = sources.iter().enumerate().filter(|(c, s)| *c == **s).count().max(1);
        let mut done: Vec<Option<Vec<f32>>> = vec![None; audio.channels.len()];
        let mut seen = 0usize;
        for (c, ch) in audio.channels.iter().enumerate() {
            if sources[c] != c {
                continue; // a copy of an earlier channel, filled in below
            }
            let x = resample(ch, sr, SR);
            let base = seen as f32;
            let report = |f: f32| progress("denoise", ((base + f) / unique as f32).min(0.99));
            let y = self.channel(&x, &report, cancel)?;
            drop(x);
            let mut out = resample(&y, SR, sr);
            out.resize(ch.len(), 0.0);
            done[c] = Some(out);
            seen += 1;
        }
        let channels = (0..audio.channels.len())
            .map(|c| done[sources[c]].clone().expect("every source channel was processed"))
            .collect();
        progress("denoise", 1.0);
        Ok(AudioData { sample_rate: sr, channels })
    }

    fn channel(&mut self, x: &[f32], report: &(dyn Fn(f32) + Sync), cancel: Cancel) -> Result<Vec<f32>> {
        let n = x.len();
        // Pad so the segments tile the signal exactly.
        let padded_len = if n < SEGMENT + STRIDE {
            SEGMENT + STRIDE
        } else {
            SEGMENT + (n - SEGMENT).div_ceil(STRIDE) * STRIDE
        };
        let mut audio: Vec<f32> = x.iter().map(|s| s * INT16).collect();
        audio.resize(padded_len, 0.0);
        let segments = (padded_len - SEGMENT) / STRIDE + 1;

        let out = Mutex::new(vec![0f32; padded_len]);
        let next = AtomicUsize::new(0);
        let finished = AtomicUsize::new(0);
        let failure: Mutex<Option<anyhow::Error>> = Mutex::new(None);
        let Self { sessions, dsp } = self;

        std::thread::scope(|scope| {
            for (w, session) in sessions.iter_mut().enumerate() {
                let (audio, out, next, finished, failure) = (&audio, &out, &next, &finished, &failure);
                let dsp = &*dsp;
                scope.spawn(move || {
                    let mut rng = 0x9e37_79b9_7f4a_7c15u64 ^ ((w as u64 + 1) << 32);
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        if k >= segments || failure.lock().unwrap().is_some() {
                            break;
                        }
                        if cancel() {
                            *failure.lock().unwrap() = Some(anyhow!("cancelled"));
                            break;
                        }
                        let start = k * STRIDE;
                        match dsp.segment(session, &audio[start..start + SEGMENT], &mut rng) {
                            Ok(seg) => {
                                let mut out = out.lock().unwrap();
                                for (i, v) in seg.iter().enumerate() {
                                    out[start + i] += v * weight(k, i);
                                }
                            }
                            Err(e) => {
                                *failure.lock().unwrap() = Some(e);
                                break;
                            }
                        }
                        let d = finished.fetch_add(1, Ordering::Relaxed) + 1;
                        report(d as f32 / segments as f32);
                    }
                });
            }
        });
        if let Some(e) = failure.into_inner().unwrap() {
            return Err(e);
        }
        let mut out = out.into_inner().unwrap();
        out.truncate(n);
        out.iter_mut().for_each(|s| *s /= INT16);
        Ok(out)
    }
}

impl Dsp {
    /// One four-second segment at integer scale, in and out.
    fn segment(&self, session: &mut Session, seg: &[f32], rng: &mut u64) -> Result<Vec<f32>> {
        let frames = 1 + (seg.len() - WIN) / HOP;
        let feats = self.features(seg, frames, rng);
        let input = Tensor::from_array(([1usize, frames, MELS * 3], feats)).map_err(|e| anyhow!("{e}"))?;
        let outputs = session.run(ort::inputs![input]).map_err(|e| anyhow!("{e}"))?;
        let (shape, mask) = outputs[0].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
        if shape.len() != 3 || shape[1] as usize != frames || shape[2] as usize != BINS {
            return Err(anyhow!("unexpected mask shape {shape:?}"));
        }

        // Mask the spectrum and overlap-add it back.
        let (mut buf, mut spec) = (self.fwd.make_input_vec(), self.fwd.make_output_vec());
        let mut inverse_in = self.inv.make_input_vec();
        let mut inverse_out = self.inv.make_output_vec();
        let mut out = vec![0f32; seg.len()];
        let mut envelope = vec![0f32; seg.len()];
        let w = &self.hamming;
        for t in 0..frames {
            let at = t * HOP;
            buf.iter_mut().zip(&seg[at..]).zip(w).for_each(|((b, s), w)| *b = s * w);
            let _ = self.fwd.process(&mut buf, &mut spec);
            for ((o, x), m) in inverse_in.iter_mut().zip(&spec).zip(&mask[t * BINS..]) {
                *o = Complex::new(x.re * m, x.im * m);
            }
            let _ = self.inv.process(&mut inverse_in, &mut inverse_out);
            for i in 0..WIN {
                out[at + i] += inverse_out[i] / WIN as f32 * w[i];
                envelope[at + i] += w[i] * w[i];
            }
        }
        out.iter_mut().zip(&envelope).for_each(|(o, e)| *o /= e.max(1e-8));
        Ok(out)
    }

    /// Kaldi-style log-mel filter bank with first and second deltas:
    /// `frames × 180`, row by row.
    fn features(&self, seg: &[f32], frames: usize, rng: &mut u64) -> Vec<f32> {
        let (mut buf, mut spec) = (self.fbank_fft.make_input_vec(), self.fbank_fft.make_output_vec());
        let mut fbank = vec![0f32; frames * MELS];
        let mut frame = vec![0f32; WIN];
        for t in 0..frames {
            frame.copy_from_slice(&seg[t * HOP..t * HOP + WIN]);
            // Dither, as in training: without it digital silence is log(0).
            frame.iter_mut().for_each(|s| *s += gauss(rng));
            let mean = frame.iter().sum::<f32>() / WIN as f32;
            frame.iter_mut().for_each(|s| *s -= mean);
            for i in (1..WIN).rev() {
                frame[i] -= 0.97 * frame[i - 1];
            }
            frame[0] -= 0.97 * frame[0];
            buf.fill(0.0);
            buf.iter_mut().zip(&frame).zip(&self.hamming).for_each(|((b, s), w)| *b = s * w);
            let _ = self.fbank_fft.process(&mut buf, &mut spec);
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
}

/// A standard normal sample (xorshift + Box–Muller).
fn gauss(seed: &mut u64) -> f32 {
    let mut next = || {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        ((*seed >> 40) as f32 + 1.0) / (1u64 << 24) as f32
    };
    let (u, v) = (next(), next());
    (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_segments_always_add_up_to_one() {
        // Every sample of a long signal, summed over the segments covering it.
        let total = SEGMENT + 5 * STRIDE;
        let mut sum = vec![0f32; total];
        for k in 0..=5 {
            for i in 0..SEGMENT {
                sum[k * STRIDE + i] += weight(k, i);
            }
        }
        // The very end belongs to a segment's discarded edge; it is padding.
        for (i, s) in sum[..total - SEGMENT + STRIDE].iter().enumerate() {
            assert!((s - 1.0).abs() < 1e-5, "sample {i} has weight {s}");
        }
    }

    #[test]
    fn the_change_from_one_segment_to_the_next_is_gradual() {
        let steps = (1..SEGMENT).map(|i| (weight(1, i) - weight(1, i - 1)).abs()).fold(0f32, f32::max);
        assert!(steps < 1e-3, "weight jumps by {steps}");
    }
}
