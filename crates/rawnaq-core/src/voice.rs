//! Where the reciter's voice is, found independently of the denoiser.
//!
//! The denoiser's own SNR estimate is a poor voice detector on old, reverberant
//! or crowded recordings: it calls the voice "noise" exactly where it is about
//! to damage it. So the voice is found by two detectors that look at the
//! untouched signal instead:
//!
//! - a small speech-detection network (Earshot, 16 ms frames at 16 kHz);
//! - a pitch detector: recitation is sung on long steady vowels, which are
//!   strongly periodic, while hiss, rumble and applause are not. This catches
//!   the long held notes a speech detector tends to let go of.
//!
//! Both are reported per 10 ms frame, 0 = no voice, 1 = voice.

use crate::resample::resample;
use cathar::AudioData;
use earshot::Detector;

/// Detector frames per second.
pub const FRAME_RATE: usize = 100;

pub struct VoiceTrack {
    /// Speech network score per 10 ms frame, 0..1.
    pub speech: Vec<f32>,
    /// Periodicity in the range of a human voice per 10 ms frame, 0..1.
    pub pitch: Vec<f32>,
    /// Level of each 10 ms frame, dBFS.
    pub level_db: Vec<f32>,
}

impl VoiceTrack {
    pub fn len(&self) -> usize {
        self.level_db.len()
    }

    pub fn is_empty(&self) -> bool {
        self.level_db.is_empty()
    }
}

pub fn detect_voice(audio: &AudioData) -> VoiceTrack {
    let n = audio.channels.first().map_or(0, |c| c.len());
    let n_ch = audio.channels.len().max(1) as f32;
    let mono: Vec<f32> =
        (0..n).map(|i| audio.channels.iter().map(|c| c[i]).sum::<f32>() / n_ch).collect();
    let x16 = resample(&mono, audio.sample_rate, 16_000);
    let n_frames = x16.len() / 160;

    let (speech, pitch) = std::thread::scope(|s| {
        let speech = s.spawn(|| speech_scores(&x16, n_frames));
        let pitch = s.spawn(|| pitch_scores(&x16, n_frames));
        (speech.join().unwrap_or_default(), pitch.join().unwrap_or_default())
    });
    let level_db = x16
        .chunks_exact(160)
        .map(|f| {
            let e = f.iter().map(|s| s * s).sum::<f32>() / 160.0;
            (10.0 * e.max(1e-12).log10()).max(-120.0)
        })
        .collect();
    VoiceTrack { speech, pitch, level_db }
}

/// Earshot works on 16 ms frames; spread its scores onto the 10 ms grid.
fn speech_scores(x16: &[f32], n_frames: usize) -> Vec<f32> {
    let mut detector = Detector::default();
    let scores: Vec<f32> = x16.chunks_exact(256).map(|f| detector.predict_f32(f)).collect();
    if scores.is_empty() {
        return vec![0.0; n_frames];
    }
    (0..n_frames)
        .map(|f| {
            let centre = f * 160 + 80;
            scores[(centre / 256).min(scores.len() - 1)]
        })
        .collect()
}

/// Peak of the normalised autocorrelation over lags of 70–450 Hz, on the
/// signal band-limited to 4 kHz. 1 = perfectly periodic.
fn pitch_scores(x16: &[f32], n_frames: usize) -> Vec<f32> {
    const WINDOW: usize = 320; // 40 ms at 8 kHz
    const MIN_LAG: usize = 18; // 444 Hz
    const MAX_LAG: usize = 114; // 70 Hz
    let x8 = resample(x16, 16_000, 8_000);
    // Remove rumble and mains hum below the voice, which are periodic too.
    let x8 = highpass(&x8, 70.0, 8_000.0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let per = n_frames.div_ceil(threads).max(1);
    let mut out = vec![0f32; n_frames];
    std::thread::scope(|s| {
        for (t, chunk) in out.chunks_mut(per).enumerate() {
            let x8 = &x8;
            s.spawn(move || {
                for (k, slot) in chunk.iter_mut().enumerate() {
                    let centre = (t * per + k) * 80 + 40;
                    let start = centre.saturating_sub(WINDOW / 2);
                    let Some(w) = x8.get(start..start + WINDOW) else { continue };
                    let span = WINDOW - MAX_LAG;
                    let e0: f32 = w[..span].iter().map(|v| v * v).sum();
                    if e0 < 1e-9 {
                        continue;
                    }
                    let mut best = 0f32;
                    for lag in MIN_LAG..=MAX_LAG {
                        let (mut r, mut e1) = (0f32, 0f32);
                        for i in 0..span {
                            r += w[i] * w[i + lag];
                            e1 += w[i + lag] * w[i + lag];
                        }
                        best = best.max(r / (e0 * e1).sqrt().max(1e-12));
                    }
                    *slot = best.clamp(0.0, 1.0);
                }
            });
        }
    });
    out
}

impl VoiceTrack {
    /// One voice score per 10 ms frame, 0 = no voice, 1 = voice.
    ///
    /// A clearly pitched frame is the reciter and scores 1. A frame the speech
    /// network hears as a voice but that has no steady pitch (a consonant, a
    /// breath, the listeners answering) scores up to 0.85. The score is then
    /// widened 50 ms before and 150 ms after every voiced frame, so word onsets
    /// and the fading tail of a long vowel are inside it, and smoothed so
    /// nothing switches abruptly.
    pub fn score(&self) -> Vec<f32> {
        let n = self.len();
        if n == 0 {
            return Vec::new();
        }
        let step = |x: f32| {
            let t = x.clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let raw: Vec<f32> = (0..n)
            .map(|f| {
                if self.level_db[f] < -70.0 {
                    return 0.0;
                }
                let pitched = step((self.pitch[f] - 0.70) / 0.18);
                let spoken = step((self.speech[f] - 0.35) / 0.30);
                pitched.max(0.85 * spoken)
            })
            .collect();
        let widened: Vec<f32> = (0..n)
            .map(|f| {
                let (a, b) = (f.saturating_sub(15), (f + 5).min(n - 1));
                raw[a..=b].iter().fold(0.0f32, |m, v| m.max(*v))
            })
            .collect();
        (0..n)
            .map(|f| {
                let (a, b) = (f.saturating_sub(2), (f + 2).min(n - 1));
                widened[a..=b].iter().sum::<f32>() / (b - a + 1) as f32
            })
            .collect()
    }
}

/// Where a denoiser cut into a voice: one value per 10 ms frame, 0 = its
/// output can be used, 1 = it dropped a voice the detector found.
///
/// Speech denoisers are trained on talking. The long held notes of a
/// recitation do not sound like talking to them, and they silence whole
/// stretches of it. Removing only noise barely changes the level of a frame
/// that holds a voice, so a frame where a voice was detected and the level
/// fell by more than a decibel or so is a frame where voice went missing.
///
/// `score` is the voice score per frame; `dry` and `wet` are the denoiser's
/// input and output.
pub fn voice_cut(dry: &AudioData, wet: &AudioData, score: &[f32]) -> Vec<f32> {
    /// Level drop (dB) where the suspicion starts, and where it is certain.
    const DROP: (f32, f32) = (1.0, 4.0);
    let hop = (dry.sample_rate as usize / FRAME_RATE).max(1);
    let n = dry.channels.first().map_or(0, |c| c.len());
    let energy = |x: &AudioData, f: usize| -> f32 {
        let (a, b) = (f * hop, ((f + 1) * hop).min(n));
        x.channels.iter().map(|c| c.get(a..b).map_or(0.0, |s| s.iter().map(|v| v * v).sum())).sum()
    };
    let raw: Vec<f32> = score
        .iter()
        .enumerate()
        .map(|(f, v)| {
            let (before, after) = (energy(dry, f), energy(wet, f));
            if before < 1e-9 {
                return 0.0;
            }
            let drop = 10.0 * (before / after.max(1e-12)).log10();
            let t = ((drop - DROP.0) / (DROP.1 - DROP.0)).clamp(0.0, 1.0);
            v * t * t * (3.0 - 2.0 * t)
        })
        .collect();
    // Start the hand-over a little before the cut and end it a little after,
    // so the change falls outside the damaged stretch, and smooth it.
    let len = raw.len();
    let widened: Vec<f32> = (0..len)
        .map(|f| {
            let (a, b) = (f.saturating_sub(8), (f + 8).min(len - 1));
            raw[a..=b].iter().fold(0.0f32, |m, v| m.max(*v))
        })
        .collect();
    (0..len)
        .map(|f| {
            let (a, b) = (f.saturating_sub(4), (f + 4).min(len - 1));
            widened[a..=b].iter().sum::<f32>() / (b - a + 1) as f32
        })
        .collect()
}

/// Stretch a per-frame score to one value per sample.
pub fn voice_mask(score: &[f32], n_samples: usize, sample_rate: u32) -> Vec<f32> {
    let n_frames = score.len();
    if n_frames == 0 {
        return vec![0.0; n_samples];
    }
    // Linear interpolation between frame centres.
    let hop = sample_rate as f32 / FRAME_RATE as f32;
    (0..n_samples)
        .map(|i| {
            let pos = (i as f32 / hop - 0.5).max(0.0);
            let f = (pos as usize).min(n_frames - 1);
            let g = (f + 1).min(n_frames - 1);
            let frac = pos - f as f32;
            score[f] * (1.0 - frac) + score[g] * frac
        })
        .collect()
}

/// Second-order Butterworth high-pass.
fn highpass(x: &[f32], cutoff: f32, sr: f32) -> Vec<f32> {
    let w = std::f32::consts::TAU * cutoff / sr;
    let alpha = w.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
    let (b0, b1, b2) = ((1.0 + w.cos()) / 2.0, -(1.0 + w.cos()), (1.0 + w.cos()) / 2.0);
    let (a0, a1, a2) = (1.0 + alpha, -2.0 * w.cos(), 1.0 - alpha);
    let (mut x1, mut x2, mut y1, mut y2) = (0f32, 0f32, 0f32, 0f32);
    x.iter()
        .map(|&s| {
            let y = (b0 * s + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2) / a0;
            (x2, x1, y2, y1) = (x1, s, y1, y);
            y
        })
        .collect()
}
