//! Decode, export, preview files and waveform peaks.

use anyhow::{anyhow, Context, Result};
use cathar::AudioData;
use serde::Serialize;
use std::path::Path;

/// Decode any supported file (WAV, MP3, FLAC, OGG, M4A, …) at its own sample rate.
pub fn load(path: &Path) -> Result<AudioData> {
    let p = path.to_str().ok_or_else(|| anyhow!("path is not valid UTF-8"))?;
    let audio = AudioData::from_file(p).with_context(|| format!("could not decode {p}"))?;
    if audio.channels.is_empty() || audio.channels[0].is_empty() {
        return Err(anyhow!("the file has no audio samples"));
    }
    Ok(audio)
}

/// Export at the audio's own sample rate. The extension picks the container:
/// `.flac` (24-bit), `.aiff` (24-bit), anything else 32-bit float WAV.
pub fn save(audio: &AudioData, path: &Path) -> Result<()> {
    let p = path.to_str().ok_or_else(|| anyhow!("path is not valid UTF-8"))?;
    audio.to_file(p).with_context(|| format!("could not write {p}"))?;
    Ok(())
}

/// 16-bit PCM WAV for in-app playback (every webview can play it).
pub fn write_preview_wav(audio: &AudioData, path: &Path) -> Result<()> {
    // Two channels that are the same signal play the same as one, in half the
    // file: for a long recording that is hundreds of megabytes less to write
    // and to load every time a setting changes.
    let sources = crate::mirror_sources(audio);
    let keep: Vec<&Vec<f32>> = audio
        .channels
        .iter()
        .enumerate()
        .filter(|(c, _)| sources[*c] == *c)
        .map(|(_, ch)| ch)
        .collect();
    let spec = hound::WavSpec {
        channels: keep.len() as u16,
        sample_rate: audio.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    let len = audio.channels[0].len();
    for i in 0..len {
        for ch in &keep {
            let s = (ch[i].clamp(-1.0, 1.0) * 32767.0).round() as i16;
            w.write_sample(s)?;
        }
    }
    w.finalize()?;
    Ok(())
}

/// What was removed: `before − after`, sample by sample. Listening to it is
/// the most direct check that the reciter's voice was left alone.
pub fn residual(before: &AudioData, after: &AudioData) -> AudioData {
    let channels = before
        .channels
        .iter()
        .zip(&after.channels)
        .map(|(b, a)| b.iter().zip(a).map(|(x, y)| x - y).collect())
        .collect();
    AudioData { sample_rate: before.sample_rate, channels }
}

/// Min/max envelope for drawing a waveform.
#[derive(Debug, Clone, Serialize)]
pub struct Peaks {
    pub min: Vec<f32>,
    pub max: Vec<f32>,
}

/// Envelope over all channels in `buckets` columns.
pub fn peaks(audio: &AudioData, buckets: usize) -> Peaks {
    peaks_of(audio, buckets, audio.channels[0].len())
}

/// The waveform of the first `len` samples only, for a signal that runs on
/// past the part being drawn (a reverb tail after the last word).
pub fn peaks_of(audio: &AudioData, buckets: usize, len: usize) -> Peaks {
    let len = len.min(audio.channels[0].len());
    let buckets = buckets.max(1).min(len.max(1));
    let mut min = vec![0.0f32; buckets];
    let mut max = vec![0.0f32; buckets];
    for b in 0..buckets {
        let start = b * len / buckets;
        let end = ((b + 1) * len / buckets).max(start + 1).min(len);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for ch in &audio.channels {
            for &s in &ch[start..end] {
                lo = lo.min(s);
                hi = hi.max(s);
            }
        }
        min[b] = lo;
        max[b] = hi;
    }
    Peaks { min, max }
}
