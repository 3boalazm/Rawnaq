//! Classic, inspectable DSP repairs (Cathar). Nothing here is generative:
//! each step only rebuilds short damaged spans from their own neighbours.

use crate::{Cancel, Progress, CANCELLED};
use cathar::{declick, declip, dehum_adaptive, inpaint_auto, AudioData};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepairSettings {
    /// Rebuild digital dropouts (runs of exact silence up to 50 ms).
    pub fill_dropouts: bool,
    /// Remove isolated clicks and pops.
    pub declick: bool,
    /// Click threshold in multiples of the local RMS; lower removes more.
    pub declick_threshold: f32,
    /// Remove 50/60 Hz mains hum. Does nothing when no hum is detected.
    pub dehum: bool,
    /// Rebuild flat-topped (clipped) peaks. Slow on long files.
    pub declip: bool,
}

impl Default for RepairSettings {
    fn default() -> Self {
        Self { fill_dropouts: true, declick: true, declick_threshold: 10.0, dehum: true, declip: false }
    }
}

impl RepairSettings {
    pub fn any(&self) -> bool {
        self.fill_dropouts || self.declick || self.dehum || self.declip
    }
}

/// Seconds per piece when a repair is applied piece by piece.
const PIECE_SECONDS: usize = 20;
/// Context on each side of a piece, seconds. A click near the cut is seen with
/// its surroundings by both neighbours.
const PIECE_CONTEXT_MS: usize = 250;

/// Apply a repair that only needs a signal's local neighbourhood (a click is
/// found and mended from the samples around it) to a long signal, piece by
/// piece, pieces side by side.
///
/// Cathar's declicker and declipper slow down far faster than the recording
/// grows: ten minutes took 38 s and an hour would take most of half an hour,
/// which is what made long recordings seem to hang. The cost is in how the
/// whole signal is searched for events, so searching twenty seconds at a time
/// makes it grow in step with the length instead.
fn in_pieces(
    x: &[f32],
    sr: u32,
    cancel: Cancel,
    repair: &(dyn Fn(&[f32]) -> Vec<f32> + Sync),
) -> anyhow::Result<Vec<f32>> {
    let piece = sr as usize * PIECE_SECONDS;
    if x.len() <= piece * 2 {
        return Ok(repair(x));
    }
    let context = sr as usize * PIECE_CONTEXT_MS / 1000;
    let n_pieces = x.len().div_ceil(piece);
    let results: std::sync::Mutex<Vec<Option<Vec<f32>>>> = std::sync::Mutex::new(vec![None; n_pieces]);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if k >= n_pieces || cancel() {
                    break;
                }
                let (start, end) = (k * piece, ((k + 1) * piece).min(x.len()));
                let (from, to) = (start.saturating_sub(context), (end + context).min(x.len()));
                let mut out = repair(&x[from..to]);
                out.resize(to - from, 0.0);
                results.lock().unwrap()[k] = Some(out[start - from..end - from].to_vec());
            });
        }
    });
    if cancel() {
        return Err(anyhow::anyhow!(CANCELLED));
    }
    let mut joined = Vec::with_capacity(x.len());
    for r in results.into_inner().unwrap() {
        joined.extend(r.expect("every piece was repaired"));
    }
    Ok(joined)
}

pub fn repair(
    audio: &AudioData,
    s: &RepairSettings,
    progress: Progress,
    cancel: Cancel,
) -> anyhow::Result<AudioData> {
    let sr = audio.sample_rate;
    let steps = [s.fill_dropouts, s.declip, s.declick, s.dehum].iter().filter(|x| **x).count();
    let sources = crate::mirror_sources(audio);
    let unique = sources.iter().enumerate().filter(|(c, s)| *c == **s).count();
    let total = (steps * unique).max(1) as f32;
    let mut done = 0.0f32;
    let mut tick = || {
        done += 1.0;
        progress("repair", done / total);
    };

    let mut channels: Vec<Vec<f32>> = Vec::with_capacity(audio.channels.len());
    for (c, ch) in audio.channels.iter().enumerate() {
        // The same signal as an earlier channel: repaired once, not twice.
        if sources[c] != c {
            let copy = channels[sources[c]].clone();
            channels.push(copy);
            continue;
        }
        let mut x = ch.clone();
        if cancel() {
            return Err(anyhow::anyhow!(CANCELLED));
        }
        if s.fill_dropouts {
            x = inpaint_auto(&x, sr, 50.0);
            tick();
        }
        if s.declip {
            x = in_pieces(&x, sr, cancel, &|piece| declip(piece, 0.99))?;
            tick();
        }
        if s.declick {
            let threshold = s.declick_threshold.clamp(4.0, 30.0);
            x = in_pieces(&x, sr, cancel, &|piece| declick(piece, threshold, 64))?;
            tick();
        }
        if s.dehum {
            // base 0 = pick 50 or 60 Hz from the recording itself.
            x = dehum_adaptive(&x, sr, 0.0, 8);
            tick();
        }
        channels.push(x);
    }
    Ok(AudioData { sample_rate: sr, channels })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    /// A repair that depends on a sample's neighbours, as a real one does: each
    /// sample becomes the mean of itself and 8 samples either side.
    fn smooth(x: &[f32]) -> Vec<f32> {
        (0..x.len())
            .map(|i| {
                let (a, b) = (i.saturating_sub(8), (i + 9).min(x.len()));
                x[a..b].iter().sum::<f32>() / (b - a) as f32
            })
            .collect()
    }

    fn signal(seconds: usize) -> Vec<f32> {
        (0..SR as usize * seconds)
            .map(|i| {
                let t = i as f32 / SR as f32;
                0.3 * (std::f32::consts::TAU * 220.0 * t).sin() + 0.1 * (std::f32::consts::TAU * 3150.0 * t).sin()
            })
            .collect()
    }

    #[test]
    fn pieces_give_exactly_what_the_whole_signal_gives() {
        // Four pieces, so there are cuts, and the repair needs context across them.
        let x = signal(75);
        let whole = smooth(&x);
        let pieces = in_pieces(&x, SR, &crate::never_cancel, &smooth).unwrap();
        assert_eq!(pieces.len(), x.len());
        let worst = pieces.iter().zip(&whole).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-6, "pieces differ from the whole by {worst}");
    }

    #[test]
    fn the_samples_at_a_cut_are_repaired_with_both_sides() {
        let x = signal(60);
        let cut = SR as usize * PIECE_SECONDS;
        let y = in_pieces(&x, SR, &crate::never_cancel, &smooth).unwrap();
        let whole = smooth(&x);
        for i in cut - 20..cut + 20 {
            assert!((y[i] - whole[i]).abs() < 1e-6, "sample {i} differs at the cut");
        }
    }

    #[test]
    fn short_signals_are_repaired_whole() {
        let x = signal(30);
        assert_eq!(in_pieces(&x, SR, &crate::never_cancel, &smooth).unwrap(), smooth(&x));
    }

    #[test]
    fn cancel_stops_a_long_repair() {
        let x = signal(75);
        assert!(in_pieces(&x, SR, &|| true, &smooth).is_err());
    }

    #[test]
    fn the_real_declicker_keeps_its_length_and_leaves_clean_audio_alone() {
        let x = signal(75);
        let y = in_pieces(&x, SR, &crate::never_cancel, &|p| declick(p, 10.0, 64)).unwrap();
        assert_eq!(y.len(), x.len());
        let worst = y.iter().zip(&x).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(worst < 0.05, "clean audio changed by {worst}");
    }
}
