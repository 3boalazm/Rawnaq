//! The space around the voice: an artificial reverberation, in a few styles.
//!
//! This is the one stage that adds something that was not in the recording.
//! It is off unless a style is chosen.
//!
//! The reverberator is a feedback delay network: eight delay lines of
//! unrelated lengths feeding each other through a mixing matrix, each losing a
//! little level and a little treble on every pass, the way sound does between
//! the walls of a room. A style is nothing more than how long the lines are
//! (the size of the room), how fast the level falls (how long the tail lasts),
//! how fast the treble falls (stone and carpet), and how late the first
//! reflection arrives (how far the walls are).

use cathar::AudioData;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReverbStyle {
    #[default]
    None,
    /// A small prayer room: short, close, intimate.
    Room,
    /// A neighbourhood mosque: the familiar recitation sound.
    Mosque,
    /// A great mosque under a dome: long and dark.
    Grand,
    /// A concert hall: long but clear and bright.
    Hall,
    /// An open, endless space: very long, airy, far away.
    Vast,
}

struct Preset {
    /// Seconds for the tail to fall by 60 dB.
    decay: f32,
    /// Delay before the reverberation starts, ms.
    predelay_ms: f32,
    /// Scale of the delay lines: the size of the space.
    size: f32,
    /// Above this the tail loses treble on every pass, Hz.
    damping_hz: f32,
    /// Below this the tail is thinned out so it does not cloud the voice, Hz.
    low_cut_hz: f32,
}

impl ReverbStyle {
    fn preset(self) -> Option<Preset> {
        let p = |decay, predelay_ms, size, damping_hz, low_cut_hz| Preset {
            decay,
            predelay_ms,
            size,
            damping_hz,
            low_cut_hz,
        };
        Some(match self {
            Self::None => return None,
            Self::Room => p(0.7, 8.0, 0.55, 6000.0, 180.0),
            Self::Mosque => p(1.9, 22.0, 1.0, 4500.0, 160.0),
            Self::Grand => p(3.4, 38.0, 1.45, 3200.0, 150.0),
            Self::Hall => p(2.3, 28.0, 1.15, 7000.0, 200.0),
            Self::Vast => p(5.5, 60.0, 1.8, 5500.0, 220.0),
        })
    }
}

/// Delay line lengths at 48 kHz and size 1: mutually prime, 30 to 78 ms.
const LINES: [usize; 8] = [1447, 1777, 2111, 2399, 2707, 3049, 3391, 3761];
/// Diffusers ahead of the network, at 48 kHz: they smear each sound into a
/// dense cluster so the tail is smooth instead of a flutter of echoes.
const DIFFUSERS: [usize; 4] = [229, 173, 613, 449];
/// Which lines feed the left and the right output, and with which sign.
const LEFT: [f32; 8] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const RIGHT: [f32; 8] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];

struct Delay {
    buf: Vec<f32>,
    at: usize,
}

impl Delay {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], at: 0 }
    }

    /// The oldest sample: what went in `len` samples ago.
    fn read(&self) -> f32 {
        self.buf[self.at]
    }

    fn write(&mut self, x: f32) {
        self.buf[self.at] = x;
        self.at = (self.at + 1) % self.buf.len();
    }
}

/// Add reverberation to `audio`.
///
/// `amount` 0..1 sets how loud the reverberation is next to the voice, from
/// barely there (−30 dB) to as loud as it can be without burying the words
/// (−6 dB). The voice itself is never turned down. The level is measured, so
/// the same amount sounds equally wet in every style.
///
/// With two channels the left and right reverberation differ, which is what
/// makes it sound like a space rather than an echo.
pub fn reverb(audio: &AudioData, style: ReverbStyle, amount: f32) -> AudioData {
    let amount = amount.clamp(0.0, 1.0);
    let Some(preset) = style.preset() else { return audio.clone() };
    let n = audio.channels.first().map_or(0, |c| c.len());
    if amount <= 0.0 || n == 0 {
        return audio.clone();
    }
    let sr = audio.sample_rate as f32;
    let scale = sr / 48_000.0;
    let n_ch = audio.channels.len();

    let mut predelay = Delay::new((preset.predelay_ms / 1000.0 * sr) as usize);
    let mut diffusers: Vec<Delay> =
        DIFFUSERS.iter().map(|d| Delay::new((*d as f32 * scale) as usize)).collect();
    let lengths: Vec<usize> =
        LINES.iter().map(|l| ((*l as f32 * scale * preset.size) as usize).max(8)).collect();
    let mut lines: Vec<Delay> = lengths.iter().map(|l| Delay::new(*l)).collect();
    // Each pass through a line must lose just enough for the whole tail to
    // fall 60 dB in `decay` seconds.
    let gains: Vec<f32> =
        lengths.iter().map(|l| 10f32.powf(-3.0 * *l as f32 / (preset.decay * sr))).collect();
    let damp = 1.0 - (-std::f32::consts::TAU * preset.damping_hz / sr).exp();
    let cut = 1.0 - (-std::f32::consts::TAU * preset.low_cut_hz / sr).exp();
    let mut dark = [0f32; 8];
    let (mut low_l, mut low_r) = (0f32, 0f32);

    let mut wet = vec![vec![0f32; n]; 2];
    let mut outs = [0f32; 8];
    for i in 0..n {
        let dry = audio.channels.iter().map(|c| c[i]).sum::<f32>() / n_ch as f32;
        let mut x = predelay.read();
        predelay.write(dry);
        for d in &mut diffusers {
            // All-pass: every frequency passes at full level, only smeared in time.
            let delayed = d.read();
            let v = x + 0.6 * delayed;
            d.write(v);
            x = delayed - 0.6 * v;
        }
        let mut sum = 0f32;
        for (k, line) in lines.iter().enumerate() {
            outs[k] = line.read();
            dark[k] += damp * (outs[k] - dark[k]);
            sum += dark[k] * gains[k];
        }
        // Householder mixing: each line is fed by all the others, losslessly.
        let spread = sum * (2.0 / 8.0);
        let (mut l, mut r) = (0f32, 0f32);
        for (k, line) in lines.iter_mut().enumerate() {
            line.write(x + dark[k] * gains[k] - spread);
            l += outs[k] * LEFT[k];
            r += outs[k] * RIGHT[k];
        }
        low_l += cut * (l - low_l);
        low_r += cut * (r - low_r);
        wet[0][i] = l - low_l;
        wet[1][i] = r - low_r;
    }

    // Set the reverberation's level against the voice's, both measured.
    let power = |x: &[f32]| x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64;
    let dry_power = audio.channels.iter().map(|c| power(c)).sum::<f64>() / n_ch as f64;
    let wet_power = (power(&wet[0]) + power(&wet[1])) / 2.0;
    if dry_power <= 0.0 || wet_power <= 0.0 {
        return audio.clone();
    }
    let target_db = -30.0 + 24.0 * amount;
    let gain = (10f64.powf(target_db as f64 / 10.0) * dry_power / wet_power).sqrt() as f32;

    let channels = audio
        .channels
        .iter()
        .enumerate()
        .map(|(c, ch)| {
            ch.iter()
                .enumerate()
                .map(|(i, s)| {
                    let w = if n_ch == 1 { (wet[0][i] + wet[1][i]) * 0.5 } else { wet[c % 2][i] };
                    s + w * gain
                })
                .collect()
        })
        .collect();
    AudioData { sample_rate: audio.sample_rate, channels }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst(sr: u32, secs: f32) -> AudioData {
        // Half a second of tone, then silence: the tail is whatever follows.
        let n = (sr as f32 * secs) as usize;
        let x = (0..n)
            .map(|i| if i < sr as usize / 2 { (i as f32 * 0.05).sin() * 0.5 } else { 0.0 })
            .collect();
        AudioData { sample_rate: sr, channels: vec![x] }
    }

    fn db(x: &[f32]) -> f32 {
        10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).max(1e-20).log10()
    }

    #[test]
    fn off_leaves_the_audio_alone() {
        let a = burst(48_000, 1.0);
        assert_eq!(reverb(&a, ReverbStyle::None, 0.8).channels, a.channels);
        assert_eq!(reverb(&a, ReverbStyle::Mosque, 0.0).channels, a.channels);
    }

    #[test]
    fn the_tail_lasts_about_as_long_as_the_style_says() {
        let sr = 48_000usize;
        let a = burst(sr as u32, 8.0);
        for (style, decay) in [(ReverbStyle::Room, 0.7f32), (ReverbStyle::Mosque, 1.9), (ReverbStyle::Grand, 3.4)] {
            let y = reverb(&a, style, 1.0).channels.remove(0);
            // Level just after the sound stops, and one decay time later.
            let at = |t: f32| db(&y[(sr as f32 * t) as usize..(sr as f32 * (t + 0.1)) as usize]);
            let fall = at(0.6) - at(0.6 + decay);
            // The treble dies sooner than the nominal time, so the measured
            // fall is at least 60 dB but not wildly more.
            assert!((45.0..90.0).contains(&fall), "{style:?} fell {fall} dB in {decay} s");
            assert!(y.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn two_channels_get_different_reverberation() {
        let mut a = burst(44_100, 2.0);
        a.channels.push(a.channels[0].clone());
        let y = reverb(&a, ReverbStyle::Hall, 0.7);
        let differ = y.channels[0].iter().zip(&y.channels[1]).any(|(l, r)| (l - r).abs() > 1e-4);
        assert!(differ);
        assert_eq!(y.channels[0].len(), a.channels[0].len());
    }
}
