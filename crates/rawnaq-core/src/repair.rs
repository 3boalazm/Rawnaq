//! Classic, inspectable DSP repairs (Cathar). Nothing here is generative:
//! each step only rebuilds short damaged spans from their own neighbours.

use crate::Progress;
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

pub fn repair(audio: &AudioData, s: &RepairSettings, progress: Progress) -> AudioData {
    let sr = audio.sample_rate;
    let steps = [s.fill_dropouts, s.declip, s.declick, s.dehum].iter().filter(|x| **x).count();
    let total = (steps * audio.channels.len()).max(1) as f32;
    let mut done = 0.0f32;
    let mut tick = || {
        done += 1.0;
        progress("repair", done / total);
    };

    let mut channels = Vec::with_capacity(audio.channels.len());
    for ch in &audio.channels {
        let mut x = ch.clone();
        if s.fill_dropouts {
            x = inpaint_auto(&x, sr, 50.0);
            tick();
        }
        if s.declip {
            x = declip(&x, 0.99);
            tick();
        }
        if s.declick {
            x = declick(&x, s.declick_threshold.clamp(4.0, 30.0), 64);
            tick();
        }
        if s.dehum {
            // base 0 = pick 50 or 60 Hz from the recording itself.
            x = dehum_adaptive(&x, sr, 0.0, 8);
            tick();
        }
        channels.push(x);
    }
    AudioData { sample_rate: sr, channels }
}
