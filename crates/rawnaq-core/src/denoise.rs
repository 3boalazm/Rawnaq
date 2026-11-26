//! DeepFilterNet3 noise suppression, CPU only, model embedded in the binary.
//!
//! The model is a streaming one: it carries its state from frame to frame, so
//! a long recitation is processed as one continuous stream and there are no
//! chunk seams to crossfade.
//!
//! It always runs at full strength here. The user-facing strength is a mix of
//! the untouched and the fully denoised signal ([`mix_protected`]), which is the
//! same thing the model's own attenuation limit does, and it means changing the
//! strength never re-runs the model.

use crate::resample::resample;
use crate::voice::VoiceMask;
use crate::{Cancel, Progress, CANCELLED};
use anyhow::{anyhow, Result};
use cathar::AudioData;
use deep_filter_rt::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Model sample rate.
const DF_SR: u32 = 48_000;

pub struct Denoised {
    /// Fully denoised audio at the source sample rate, aligned with the input.
    pub wet: AudioData,
    /// The model's local SNR estimate (dB) per 10 ms frame, per channel. Kept
    /// for diagnostics only: on old recordings it calls the voice noise.
    pub lsnr: Vec<Vec<f32>>,
}

pub fn denoise_full(audio: &AudioData, progress: Progress, cancel: Cancel) -> Result<Denoised> {
    let sr = audio.sample_rate;
    let n_ch = audio.channels.len();
    let sources = crate::mirror_sources(audio);
    let done = AtomicUsize::new(0);
    let total_frames: usize = audio
        .channels
        .iter()
        .enumerate()
        .filter(|(c, _)| sources[*c] == *c)
        .map(|(_, c)| (c.len() as u64 * DF_SR as u64 / sr as u64) as usize / 480 + 4)
        .sum();

    // One independent model instance per distinct channel, channels in parallel.
    let results: Vec<Option<Result<(Vec<f32>, Vec<f32>)>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = audio
            .channels
            .iter()
            .enumerate()
            .map(|(c, ch)| {
                (sources[c] == c).then(|| {
                    let done = &done;
                    scope.spawn(move || denoise_channel(ch, sr, done, total_frames, progress, cancel))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.map(|h| h.join().unwrap_or_else(|_| Err(anyhow!("denoise thread panicked")))))
            .collect()
    });

    let mut computed: Vec<Option<(Vec<f32>, Vec<f32>)>> = Vec::with_capacity(n_ch);
    for r in results {
        computed.push(match r {
            Some(r) => Some(r?),
            None => None,
        });
    }
    let mut channels = Vec::with_capacity(n_ch);
    let mut lsnr = Vec::with_capacity(n_ch);
    for c in 0..n_ch {
        let (wet, l) = computed[sources[c]].as_ref().expect("every source channel was processed");
        channels.push(wet.clone());
        lsnr.push(l.clone());
    }
    progress("denoise", 1.0);
    Ok(Denoised { wet: AudioData { sample_rate: sr, channels }, lsnr })
}

fn denoise_channel(
    input: &[f32],
    sr: u32,
    done: &AtomicUsize,
    total_frames: usize,
    progress: Progress,
    cancel: Cancel,
) -> Result<(Vec<f32>, Vec<f32>)> {
    let x48 = resample(input, sr, DF_SR);

    let mut df = DfTract::new(DfParams::default(), &RuntimeParams::default_with_ch(1))?;
    let hop = df.hop_size;
    let delay = df.delay();

    // The output lags the input by `delay` samples: feed that much extra
    // silence at the end and drop the same amount from the start.
    let padded_len = (x48.len() + delay).div_ceil(hop) * hop;
    let mut padded = x48;
    let n48 = padded.len();
    padded.resize(padded_len, 0.0);

    let n_frames = padded_len / hop;
    let mut out = Vec::with_capacity(padded_len);
    let mut lsnr = Vec::with_capacity(n_frames);
    let mut noisy = Array2::<f32>::zeros((1, hop));
    let mut enh = Array2::<f32>::zeros((1, hop));

    for (f, frame) in padded.chunks_exact(hop).enumerate() {
        if f % 200 == 0 && cancel() {
            return Err(anyhow!(CANCELLED));
        }
        noisy.row_mut(0).iter_mut().zip(frame).for_each(|(d, s)| *d = *s);
        let l = df.process(noisy.view(), enh.view_mut())?;
        lsnr.push(l);
        out.extend(enh.row(0).iter().copied());
        if f % 200 == 199 {
            let d = done.fetch_add(200, Ordering::Relaxed) + 200;
            progress("denoise", (d as f32 / total_frames as f32).min(0.99));
        }
    }

    let aligned = &out[delay..delay + n48];
    let mut wet = resample(aligned, DF_SR, sr);
    wet.resize(input.len(), 0.0);

    // Frame k of the output describes the input `delay` samples earlier.
    let shift = delay / hop;
    let lsnr = lsnr.into_iter().skip(shift).collect();
    Ok((wet, lsnr))
}

/// Merge the mask into spans of voice, in seconds, for display.
pub fn voice_regions(mask: &VoiceMask, _sample_rate: u32) -> Vec<[f32; 2]> {
    let frames = mask.frames();
    let mut out: Vec<[f32; 2]> = Vec::new();
    let mut start: Option<usize> = None;
    for (k, v) in frames.iter().enumerate() {
        match (*v > 0.5, start) {
            (true, None) => start = Some(k),
            (false, Some(s)) => {
                out.push([s as f32 * 0.01, k as f32 * 0.01]);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push([s as f32 * 0.01, frames.len() as f32 * 0.01]);
    }
    out.truncate(2000);
    out
}

/// Mix the untouched signal with the two denoisers.
///
/// `hiss` 0..1 is the share of the profile-denoised signal (`clean`), applied
/// everywhere: it only lowers the measured noise floor, so it is safe under
/// the voice.
///
/// `strength` 0..1 is the share of the neural denoiser's output (`wet`) on top
/// of that; it is equivalent to capping the model's attenuation at
/// `-20·log10(1 − strength)` dB.
///
/// `protect` 0..1 lowers that strength where `mask` says the voice is, down to
/// `strength · (1 − protect)`. Between phrases the full strength still applies,
/// so the gaps get clean while the voice keeps its body.
pub fn mix_protected(
    dry: &AudioData,
    clean: &AudioData,
    wet: &AudioData,
    hiss: f32,
    strength: f32,
    protect: f32,
    mask: &VoiceMask,
) -> AudioData {
    let h = hiss.clamp(0.0, 1.0);
    let s = strength.clamp(0.0, 1.0);
    let p = protect.clamp(0.0, 1.0);
    let channels = dry
        .channels
        .iter()
        .zip(&clean.channels)
        .zip(&wet.channels)
        .map(|((d, c), w)| {
            d.iter()
                .zip(c)
                .zip(w)
                .enumerate()
                .map(|(i, ((d, c), w))| {
                    let base = d + (c - d) * h;
                    let m = s * (1.0 - p * mask.at(i));
                    base * (1.0 - m) + w * m
                })
                .collect()
        })
        .collect();
    AudioData { sample_rate: dry.sample_rate, channels }
}
