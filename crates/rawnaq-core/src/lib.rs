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
pub use voice::{detect_voice, voice_cut, voice_mask, VoiceTrack};

/// Progress callback: a stage id and a fraction in `0.0..=1.0`.
pub type Progress<'a> = &'a (dyn Fn(&str, f32) + Sync);

/// A progress callback that does nothing.
pub fn no_progress(_: &str, _: f32) {}
