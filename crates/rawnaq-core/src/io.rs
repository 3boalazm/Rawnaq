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
    let spec = hound::WavSpec {
        channels: audio.channels.len() as u16,
        sample_rate: audio.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    let len = audio.channels[0].len();
    for i in 0..len {
        for ch in &audio.channels {
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
    let len = audio.channels[0].len();
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
