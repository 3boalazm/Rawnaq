//! Quality gate: did the denoiser take anything from the voice?
//!
//! Heuristic, not proof. It compares the level before and after in 10 ms
//! frames, using the voice detector's score to tell voice frames from noise
//! frames:
//!
//! - In a voice frame the voice carries most of the energy, so removing the
//!   noise under it lowers the level only a little. A big drop in such a
//!   frame means voice energy went with it.
//! - In frames without a voice the drop is the intended noise reduction.
//!
//! The reliable check is still a human listening to the removed signal.

use cathar::AudioData;
use serde::Serialize;

/// Voice score at or above which a frame counts as voice.
const VOICE_SCORE: f32 = 0.6;
/// Voice score at or below which a frame counts as noise only.
const NOISE_SCORE: f32 = 0.15;
/// A voice frame losing more than this many dB is flagged.
const FLAG_LOSS_DB: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Voice level essentially unchanged.
    Ok,
    /// Small but measurable loss in voice frames: listen before accepting.
    Caution,
    /// Clear loss in voice frames: lower the strength.
    Warn,
    /// Too few voice frames found to judge.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct QualityReport {
    pub verdict: Verdict,
    /// Level drop over all voice frames, dB (0 = nothing lost).
    pub voice_loss_db: f32,
    /// Share of voice frames that lost more than 3 dB, 0..1.
    pub flagged_ratio: f32,
    /// Level drop over noise-only frames, dB: the actual noise reduction.
    pub noise_reduction_db: f32,
    /// Share of frames classed as voice, 0..1.
    pub voice_ratio: f32,
    /// Flagged spans in seconds `[start, end]`, merged, at most 200.
    pub flagged_regions: Vec<[f32; 2]>,
}

/// `before` is the denoiser's input, `after` its (mixed) output, same length
/// and sample rate. `score` is the voice score, one value per 10 ms.
pub fn quality_gate(before: &AudioData, after: &AudioData, score: &[f32]) -> QualityReport {
    let sr = before.sample_rate as usize;
    let n = before.channels[0].len();
    let n_frames = score.len();

    let (mut v_in, mut v_out, mut z_in, mut z_out) = (0f64, 0f64, 0f64, 0f64);
    let (mut voice_frames, mut flagged_frames, mut frames) = (0usize, 0usize, 0usize);
    let mut flags = vec![false; n_frames];
    // (frame, energy before, energy after) of every voice frame.
    let mut voice: Vec<(usize, f64, f64)> = Vec::new();

    for f in 0..n_frames {
        // 10 ms frames at the source rate, matching the model's frames in time.
        let start = f * sr / 100;
        let end = ((f + 1) * sr / 100).min(n);
        if start >= end {
            break;
        }
        frames += 1;
        // A frame counts as voice only if its neighbours do too, and as noise
        // only if they do too: word edges, where a 10 ms frame is half voice
        // and half silence, are left out of both groups.
        let (a, b) = (f.saturating_sub(1), (f + 1).min(n_frames - 1));
        let (lo, hi) =
            score[a..=b].iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let (is_voice, is_noise) = (lo >= VOICE_SCORE, hi <= NOISE_SCORE);
        let (mut e_in, mut e_out) = (0f64, 0f64);
        for (b, a) in before.channels.iter().zip(&after.channels) {
            for i in start..end {
                e_in += (b[i] as f64).powi(2);
                e_out += (a[i] as f64).powi(2);
            }
        }
        if e_in < 1e-10 {
            continue; // digital silence
        }
        if is_voice {
            voice_frames += 1;
            v_in += e_in;
            v_out += e_out;
            voice.push((f, e_in, e_out));
        } else if is_noise {
            z_in += e_in;
            z_out += e_out;
        }
    }

    // Flag only frames that carry real voice level: within 30 dB of the loud
    // voice frames. Below that, a "voice" frame is a breath or the gap between
    // words, and removing what little is there is not a loss of recitation.
    let mut levels: Vec<f64> = voice.iter().map(|v| v.1).collect();
    levels.sort_by(|a, b| a.total_cmp(b));
    let loud = levels.get(levels.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0.0);
    let audible = loud * 1e-3;
    let mut judged = 0usize;
    for &(f, e_in, e_out) in &voice {
        if e_in < audible {
            continue;
        }
        judged += 1;
        if db(e_in, e_out) > FLAG_LOSS_DB {
            flagged_frames += 1;
            flags[f] = true;
        }
    }

    let voice_ratio = if frames > 0 { voice_frames as f32 / frames as f32 } else { 0.0 };
    let voice_loss_db = if voice_frames > 0 { db(v_in, v_out).max(0.0) } else { 0.0 };
    let flagged_ratio = if judged > 0 { flagged_frames as f32 / judged as f32 } else { 0.0 };
    let noise_reduction_db = if z_in > 0.0 { db(z_in, z_out).max(0.0) } else { 0.0 };

    let verdict = if voice_frames < 50 {
        Verdict::Unknown
    } else if voice_loss_db > 1.5 || flagged_ratio > 0.10 {
        Verdict::Warn
    } else if voice_loss_db > 0.8 || flagged_ratio > 0.03 {
        Verdict::Caution
    } else {
        Verdict::Ok
    };

    QualityReport {
        verdict,
        voice_loss_db,
        flagged_ratio,
        noise_reduction_db,
        voice_ratio,
        flagged_regions: merge_regions(&flags),
    }
}

fn db(e_in: f64, e_out: f64) -> f32 {
    (10.0 * (e_in / e_out.max(1e-20)).log10()).min(120.0) as f32
}

/// Merge flagged 10 ms frames closer than 100 ms into spans.
fn merge_regions(flags: &[bool]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::new();
    for (f, _) in flags.iter().enumerate().filter(|(_, x)| **x) {
        let (s, e) = (f as f32 * 0.01, (f + 1) as f32 * 0.01);
        match out.last_mut() {
            Some(last) if s - last[1] <= 0.1 => last[1] = e,
            _ => out.push([s, e]),
        }
    }
    out.truncate(200);
    out
}
