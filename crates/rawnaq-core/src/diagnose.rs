//! Measure the recording before touching it.

use cathar::{compute_stats, detect_mains_hz, AudioData};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Diagnosis {
    pub sample_rate: u32,
    pub channels: usize,
    pub duration_sec: f32,
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
    pub loudness_lufs: f32,
    /// Mean level of the quietest 15% of frames.
    pub noise_floor_dbfs: f32,
    pub snr_db: f32,
    pub clipped_samples: usize,
    pub clipped_runs: usize,
    /// 50 or 60 when mains hum stands out, otherwise `None`.
    pub mains_hum_hz: Option<f32>,
    /// Runs of exact digital silence up to 50 ms inside the recording.
    pub dropouts: usize,
    /// Issue ids the UI turns into advice: `noise`, `clipping`, `hum`, `dropouts`.
    pub issues: Vec<String>,
}

pub fn diagnose(audio: &AudioData) -> Diagnosis {
    let sr = audio.sample_rate;
    let stats = compute_stats(&audio.channels, sr);
    let mono = downmix(audio);
    let mains_hum_hz = detect_mains_hz(&mono, sr);
    let dropouts = count_dropouts(&mono, sr, 50.0);

    let mut issues = Vec::new();
    let mut d = Diagnosis {
        sample_rate: sr,
        channels: audio.channels.len(),
        duration_sec: mono.len() as f32 / sr as f32,
        peak_dbfs: 0.0,
        rms_dbfs: 0.0,
        loudness_lufs: 0.0,
        noise_floor_dbfs: 0.0,
        snr_db: 0.0,
        clipped_samples: 0,
        clipped_runs: 0,
        mains_hum_hz,
        dropouts,
        issues: Vec::new(),
    };
    if let Some(s) = stats {
        for sug in s.suggestions() {
            if sug.issue == "noise" || sug.issue == "clipping" {
                issues.push(sug.issue.to_string());
            }
        }
        d.peak_dbfs = finite(s.peak_dbfs);
        d.rms_dbfs = finite(s.rms_dbfs);
        d.loudness_lufs = finite(s.integrated_lufs);
        d.noise_floor_dbfs = finite(s.noise_floor_dbfs);
        d.snr_db = finite(s.snr_db);
        d.clipped_samples = s.clipped_samples;
        d.clipped_runs = s.clipped_runs;
    }
    if mains_hum_hz.is_some() {
        issues.push("hum".into());
    }
    if dropouts > 0 {
        issues.push("dropouts".into());
    }
    d.issues = issues;
    d
}

/// JSON has no infinity; a silent file would otherwise serialise to `null`.
fn finite(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        -120.0
    }
}

pub(crate) fn downmix(audio: &AudioData) -> Vec<f32> {
    let n = audio.channels[0].len();
    let k = audio.channels.len() as f32;
    (0..n).map(|i| audio.channels.iter().map(|c| c[i]).sum::<f32>() / k).collect()
}

/// Interior runs of exact zeros between 1 ms and `max_gap_ms`: the digital
/// dropouts that `repair` can reconstruct.
fn count_dropouts(signal: &[f32], sr: u32, max_gap_ms: f32) -> usize {
    let min_gap = (sr as usize / 1000).max(2);
    let max_gap = ((max_gap_ms / 1000.0) * sr as f32) as usize;
    let n = signal.len();
    let (mut count, mut i) = (0, 0);
    while i < n {
        if signal[i] == 0.0 {
            let start = i;
            while i < n && signal[i] == 0.0 {
                i += 1;
            }
            let len = i - start;
            if start > 0 && i < n && len >= min_gap && len <= max_gap {
                count += 1;
            }
        } else {
            i += 1;
        }
    }
    count
}
