//! Rawnaq audio core.
//!
//! Pipeline: decode → diagnose → repair (classic DSP, Cathar) → detect the
//! voice (Earshot + pitch) and measure the noise → denoise (noise-profile
//! subtraction everywhere, DeepFilterNet3 where the voice is not) → quality
//! gate → enhance (tone, loudness) → export.
//!
//! Every stage returns a new buffer and never touches its input, so the caller
//! can cache each stage and re-run only what changed.

pub mod body;
pub mod denoise;
pub mod diagnose;
pub mod enhance;
pub mod io;
pub mod mossformer;
pub mod noise;
pub mod quality;
pub mod repair;
pub mod resample;
pub mod reverb;
pub mod voice;

pub use body::{body, measure, BodyStyle, ToneBalance};
pub use cathar::AudioData;
pub use denoise::{denoise_full, mix_protected, voice_regions, Denoised};
pub use diagnose::{diagnose, Diagnosis};
pub use enhance::{enhance, tone_loss, EnhanceReport, EnhanceSettings};
pub use noise::{learn_noise, subtract_noise, NoiseProfile};
pub use io::{load, peaks, peaks_of, residual, save, write_preview_wav, Peaks};
pub use quality::{quality_gate, QualityReport, Verdict};
pub use repair::{repair, RepairSettings};
pub use reverb::{reverb, ReverbStyle};
pub use voice::{detect_voice, voice_cut, voice_mask, voiced_energy, VoiceMask, VoiceTrack};

/// Progress callback: a stage id and a fraction in `0.0..=1.0`.
pub type Progress<'a> = &'a (dyn Fn(&str, f32) + Sync);

/// A progress callback that does nothing.
pub fn no_progress(_: &str, _: f32) {}

/// Asked now and then by the long stages: `true` means stop and return the
/// error `"cancelled"`.
pub type Cancel<'a> = &'a (dyn Fn() -> bool + Sync);

/// A cancel check that never cancels.
pub fn never_cancel() -> bool {
    false
}

/// The error text a cancelled stage returns.
pub const CANCELLED: &str = "cancelled";

/// For each channel, the channel it can be processed as: itself, or an earlier
/// channel it is a copy of.
///
/// Old mono recordings are very often saved as stereo, with the same signal on
/// both sides. Processing the second side again would double the time of the
/// heaviest stages and say nothing new. "A copy" means the difference between
/// the two is more than 40 dB below the signal, which is far below anything
/// that could be heard or that a denoiser would treat differently.
pub fn mirror_sources(audio: &AudioData) -> Vec<usize> {
    let n = audio.channels.first().map_or(0, |c| c.len());
    let mut sources: Vec<usize> = (0..audio.channels.len()).collect();
    for c in 1..audio.channels.len() {
        for s in 0..c {
            if sources[s] != s || audio.channels[c].len() != n {
                continue;
            }
            // Every 5th sample is plenty to tell a copy from a different signal.
            let (mut signal, mut diff) = (0f64, 0f64);
            for i in (0..n).step_by(5) {
                let (a, b) = (audio.channels[s][i] as f64, audio.channels[c][i] as f64);
                signal += a * a;
                diff += (a - b) * (a - b);
            }
            if signal > 0.0 && diff <= signal * 1e-4 {
                sources[c] = s;
                break;
            }
        }
    }
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amp: f32) -> Vec<f32> {
        (0..44_100).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / 44_100.0).sin()).collect()
    }

    fn stereo(l: Vec<f32>, r: Vec<f32>) -> AudioData {
        AudioData { sample_rate: 44_100, channels: vec![l, r] }
    }

    #[test]
    fn a_copied_channel_is_processed_once() {
        let a = tone(440.0, 0.3);
        assert_eq!(mirror_sources(&stereo(a.clone(), a.clone())), vec![0, 0]);
    }

    #[test]
    fn a_channel_that_is_almost_the_same_counts_as_a_copy() {
        // The difference is 50 dB under the signal.
        let a = tone(440.0, 0.3);
        let b: Vec<f32> = a.iter().zip(tone(3000.0, 0.3 * 0.003)).map(|(x, y)| x + y).collect();
        assert_eq!(mirror_sources(&stereo(a, b)), vec![0, 0]);
    }

    #[test]
    fn different_channels_are_both_processed() {
        assert_eq!(mirror_sources(&stereo(tone(440.0, 0.3), tone(660.0, 0.3))), vec![0, 1]);
        // Silence is never "the same" as a signal.
        assert_eq!(mirror_sources(&stereo(tone(440.0, 0.3), vec![0.0; 44_100])), vec![0, 1]);
    }

    #[test]
    fn a_third_channel_copies_the_first() {
        let a = tone(440.0, 0.3);
        let audio = AudioData { sample_rate: 44_100, channels: vec![a.clone(), tone(660.0, 0.3), a] };
        assert_eq!(mirror_sources(&audio), vec![0, 1, 0]);
    }
}
