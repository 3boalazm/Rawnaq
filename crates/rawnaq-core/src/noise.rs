//! Noise detector and profile denoiser.
//!
//! Tape hiss, room tone and electrical noise are steady: their spectrum barely
//! changes over a recording, while the voice moves all the time. So the noise
//! is measured once, as a spectrum, from the level each frequency falls back
//! to when nothing else is there ([`learn_noise`]), and then only what sits at that measured floor is turned
//! down ([`subtract_noise`]). Anything clearly above the floor, which is where
//! the voice lives, passes unchanged. That is what makes this engine safe to
//! run *under* the voice, where the neural denoiser is held back.

use cathar::AudioData;
use realfft::RealFftPlanner;
use serde::Serialize;

const FFT: usize = 2048;
const HOP: usize = FFT / 4;
const BINS: usize = FFT / 2 + 1;
/// Frames quieter than this are digital silence and say nothing about noise.
const SILENCE_DB: f32 = -80.0;
/// The measured floor is scaled up by this before subtracting, so noise that
/// happens to peak above its average is still caught.
const OVER: f32 = 2.0;
/// Deepest cut at full strength: −15 dB. Never to zero, which sounds dead.
const FLOOR: f32 = 0.178;

#[derive(Debug, Clone, Serialize)]
pub struct NoiseProfile {
    /// Mean noise power per frequency bin of a 2048-point transform.
    #[serde(skip)]
    pub power: Vec<f32>,
    /// Overall level of the steady noise, dBFS.
    pub level_dbfs: f32,
    /// Level below 250 Hz (rumble, hum), dBFS.
    pub low_dbfs: f32,
    /// Level from 250 Hz to 3 kHz (room, crowd murmur), dBFS.
    pub mid_dbfs: f32,
    /// Level above 3 kHz (hiss), dBFS.
    pub high_dbfs: f32,
    /// The band that carries most of the noise: "low", "mid" or "high".
    pub kind: &'static str,
}

fn window() -> Vec<f32> {
    // Square-root Hann: applied before and after the transform, the two
    // together overlap-add to a constant at 75 % overlap.
    (0..FFT)
        .map(|i| (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT as f32).cos()).sqrt())
        .collect()
}

/// Measure the steady noise.
///
/// Each frequency is looked at on its own, across the whole recording, and the
/// level it falls back to in its quietest tenth of the time is taken as its
/// noise. A voice moves from note to note, so every frequency is free of it
/// some of the time, even in a recording with no silent gap at all.
pub fn learn_noise(audio: &AudioData) -> NoiseProfile {
    /// Share of the time a frequency is assumed to hold nothing but noise.
    const QUIET_SHARE: f32 = 0.10;
    /// Frames examined at most, spread evenly over the recording.
    const MAX_FRAMES: usize = 3000;

    let sr = audio.sample_rate as f32;
    let n = audio.channels.first().map_or(0, |c| c.len());
    let n_frames = if n >= FFT { (n - FFT) / HOP + 1 } else { 0 };
    let n_ch = audio.channels.len().max(1) as f32;

    let audible: Vec<usize> = (0..n_frames)
        .filter(|f| {
            let s = f * HOP;
            let e: f32 = audio
                .channels
                .iter()
                .map(|c| c[s..s + FFT].iter().map(|v| v * v).sum::<f32>())
                .sum::<f32>()
                / (FFT as f32 * n_ch);
            10.0 * e.max(1e-12).log10() > SILENCE_DB
        })
        .collect();
    let stride = audible.len().div_ceil(MAX_FRAMES).max(1);
    let chosen: Vec<usize> = audible.into_iter().step_by(stride).collect();

    let mut power = vec![0f32; BINS];
    if !chosen.is_empty() {
        let win = window();
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT);
        let (mut buf, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
        // Per bin: its power in every examined frame.
        let mut columns = vec![Vec::with_capacity(chosen.len()); BINS];
        for &f in &chosen {
            let mut frame = vec![0f32; BINS];
            for c in &audio.channels {
                buf.iter_mut().zip(&c[f * HOP..]).zip(&win).for_each(|((b, s), w)| *b = s * w);
                let _ = fft.process(&mut buf, &mut spec);
                frame.iter_mut().zip(&spec).for_each(|(p, x)| *p += x.norm_sqr() / n_ch);
            }
            columns.iter_mut().zip(&frame).for_each(|(col, p)| col.push(*p));
        }
        // Noise power in one bin is exponentially distributed: the value it
        // stays under a share q of the time is −ln(1 − q) times its mean.
        let to_mean = 1.0 / -(1.0 - QUIET_SHARE).ln();
        for (p, col) in power.iter_mut().zip(&mut columns) {
            col.sort_by(|a, b| a.total_cmp(b));
            *p = col[(col.len() as f32 * QUIET_SHARE) as usize] * to_mean;
        }
    }

    // Parseval with this window: variance = 4 · Σ power / FFT².
    let band = |lo: f32, hi: f32| {
        let sum: f32 = power
            .iter()
            .enumerate()
            .filter(|(k, _)| {
                let hz = *k as f32 * sr / FFT as f32;
                hz >= lo && hz < hi
            })
            .map(|(_, p)| *p)
            .sum();
        (10.0 * (4.0 * sum / (FFT * FFT) as f32).max(1e-12).log10()).max(-120.0)
    };
    let (low, mid, high) = (band(0.0, 250.0), band(250.0, 3000.0), band(3000.0, sr));
    let level = band(0.0, sr);
    let kind = if low >= mid && low >= high {
        "low"
    } else if high >= mid {
        "high"
    } else {
        "mid"
    };
    NoiseProfile {
        power,
        level_dbfs: level,
        low_dbfs: low,
        mid_dbfs: mid,
        high_dbfs: high,
        kind,
    }
}

/// Turn down, at full strength, whatever sits at the measured noise floor.
/// The caller mixes the result with the untouched signal for less strength.
pub fn subtract_noise(audio: &AudioData, profile: &NoiseProfile) -> AudioData {
    let channels = std::thread::scope(|s| {
        let handles: Vec<_> = audio
            .channels
            .iter()
            .map(|c| s.spawn(move || subtract_channel(c, &profile.power)))
            .collect();
        handles
            .into_iter()
            .zip(&audio.channels)
            .map(|(h, c)| h.join().unwrap_or_else(|_| c.clone()))
            .collect()
    });
    AudioData { sample_rate: audio.sample_rate, channels }
}

fn subtract_channel(x: &[f32], noise: &[f32]) -> Vec<f32> {
    let n = x.len();
    if n < FFT || noise.iter().all(|p| *p <= 0.0) {
        return x.to_vec();
    }
    let win = window();
    let mut planner = RealFftPlanner::<f32>::new();
    let (fwd, inv) = (planner.plan_fft_forward(FFT), planner.plan_fft_inverse(FFT));
    let (mut buf, mut spec) = (fwd.make_input_vec(), fwd.make_output_vec());
    // Pad so the first and last samples are covered by four frames like the rest.
    let pad = FFT - HOP;
    let total = (n + 2 * pad).div_ceil(HOP) * HOP + FFT;
    let mut padded = vec![0f32; total];
    padded[pad..pad + n].copy_from_slice(x);
    let mut out = vec![0f32; total];

    let mut held = vec![0f32; BINS]; // signal power, smoothed over time
    let mut power = vec![0f32; BINS];
    let mut gain = vec![0f32; BINS];
    // The window applied twice is a Hann window, which sums to 2 at 75 %
    // overlap; the inverse transform is unnormalised.
    let scale = 1.0 / (2.0 * FFT as f32);

    for start in (0..=total - FFT).step_by(HOP) {
        buf.iter_mut().zip(&padded[start..]).zip(&win).for_each(|((b, s), w)| *b = s * w);
        let _ = fwd.process(&mut buf, &mut spec);
        for (p, x) in power.iter_mut().zip(&spec) {
            *p = x.norm_sqr();
        }
        for k in 0..BINS {
            // Average over neighbouring bins and over time: a lone bin
            // that pokes above the floor for one frame is noise, not voice,
            // and is what makes a denoiser "twinkle".
            let (a, b) = (k.saturating_sub(2), (k + 2).min(BINS - 1));
            let local = power[a..=b].iter().sum::<f32>() / (b - a + 1) as f32;
            // Quick to rise (a voice starting is let through at once), slow
            // to fall (its tail is not chopped).
            let rise = if local > held[k] { 0.6 } else { 0.2 };
            held[k] += rise * (local - held[k]);
            let floor = OVER * noise[k];
            gain[k] = (1.0 - floor / held[k].max(1e-20)).max(0.0).sqrt().max(FLOOR);
        }
        for (x, g) in spec.iter_mut().zip(&gain) {
            *x *= *g;
        }
        let _ = inv.process(&mut spec, &mut buf);
        out[start..start + FFT]
            .iter_mut()
            .zip(&buf)
            .zip(&win)
            .for_each(|((o, s), w)| *o += s * w * scale);
    }
    out[pad..pad + n].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut seed = 0x2545_f491u32;
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn rms_db(x: &[f32]) -> f32 {
        10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).log10()
    }

    #[test]
    fn removes_hiss_and_keeps_the_tone() {
        let sr = 44_100usize;
        let hiss = noise(sr * 4, 0.01);
        // A loud tone in the second half only.
        let x: Vec<f32> = hiss
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let tone = if i >= sr * 2 { 0.3 * (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() } else { 0.0 };
                h + tone
            })
            .collect();
        let audio = AudioData { sample_rate: sr as u32, channels: vec![x.clone()] };
        let profile = learn_noise(&audio);
        // Uniform noise of amplitude 0.01 has an RMS of −44.8 dBFS.
        assert!((profile.level_dbfs + 44.8).abs() < 1.5, "level {}", profile.level_dbfs);
        let y = subtract_noise(&audio, &profile).channels.remove(0);
        let quiet = sr / 2..sr * 3 / 2;
        let cut = rms_db(&x[quiet.clone()]) - rms_db(&y[quiet]);
        assert!(cut > 10.0, "hiss cut by only {cut} dB");
        let loud = sr * 5 / 2..sr * 7 / 2;
        let change = (rms_db(&x[loud.clone()]) - rms_db(&y[loud])).abs();
        assert!(change < 0.2, "tone changed by {change} dB");
    }

    #[test]
    fn silence_and_short_input_pass_through() {
        let audio = AudioData { sample_rate: 44_100, channels: vec![vec![0.0; 100]] };
        let p = learn_noise(&audio);
        assert_eq!(subtract_noise(&audio, &p).channels[0].len(), 100);
    }
}
