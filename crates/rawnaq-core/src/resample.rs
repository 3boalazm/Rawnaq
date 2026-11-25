//! Sample-rate conversion: Kaiser-windowed sinc as a precomputed polyphase
//! filter bank. Same filter design as Cathar's resampler (16 lobes, β = 9) but
//! the kernel is computed once per phase instead of once per sample, which is
//! what makes hour-long recordings practical.

/// Largest filter bank worth precomputing; odd rate pairs fall back to Cathar.
const MAX_PHASES: u64 = 2048;
const LOBES: f64 = 16.0;
const BETA: f64 = 9.0;

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0f64, 1.0f64);
    let half_sq = (x / 2.0) * (x / 2.0);
    for k in 1..=30 {
        term *= half_sq / (k as f64 * k as f64);
        sum += term;
        if term < 1e-13 * sum {
            break;
        }
    }
    sum
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

pub fn resample(signal: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || from_rate == 0 || to_rate == 0 || signal.is_empty() {
        return signal.to_vec();
    }
    let g = gcd(from_rate as u64, to_rate as u64);
    let (m, l) = (from_rate as u64 / g, to_rate as u64 / g); // output i sits at source i·m/l
    if l > MAX_PHASES {
        return cathar::resample(signal, from_rate, to_rate);
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let cutoff = 0.5 * ratio.min(1.0); // cycles per source sample
    let half_width = LOBES / (2.0 * cutoff);
    let h = half_width.ceil() as i64; // taps cover source offsets -h+1 ..= h
    let taps = (2 * h) as usize;
    let i0_beta = bessel_i0(BETA);

    // bank[phase][j] weights source sample n + (j - h + 1) for fraction phase/l.
    let mut bank = vec![0.0f32; l as usize * taps];
    for phase in 0..l as usize {
        let frac = phase as f64 / l as f64;
        let row = &mut bank[phase * taps..(phase + 1) * taps];
        let mut sum = 0.0f64;
        let mut tmp = vec![0.0f64; taps];
        for (j, t) in tmp.iter_mut().enumerate() {
            let dx = frac - (j as i64 - h + 1) as f64; // centre − source index
            let sinc = if dx.abs() < 1e-9 {
                2.0 * cutoff
            } else {
                (std::f64::consts::PI * 2.0 * cutoff * dx).sin() / (std::f64::consts::PI * dx)
            };
            let r = dx / half_width;
            let w = if r.abs() < 1.0 { bessel_i0(BETA * (1.0 - r * r).sqrt()) / i0_beta } else { 0.0 };
            *t = sinc * w;
            sum += *t;
        }
        for (o, t) in row.iter_mut().zip(&tmp) {
            *o = (*t / sum) as f32; // unity gain at DC for every phase
        }
    }

    let n = signal.len() as i64;
    let out_len = (signal.len() as f64 * ratio).round().max(1.0) as usize;
    let mut out = vec![0.0f32; out_len];
    for (i, o) in out.iter_mut().enumerate() {
        let pos = i as u64 * m;
        let base = (pos / l) as i64 - h + 1;
        let row = &bank[(pos % l) as usize * taps..][..taps];
        let (lo, hi) = (base.max(0), (base + taps as i64).min(n));
        let mut acc = 0.0f32;
        if lo < hi {
            let src = &signal[lo as usize..hi as usize];
            let w = &row[(lo - base) as usize..][..src.len()];
            for (s, k) in src.iter().zip(w) {
                acc += s * k;
            }
        }
        *o = acc;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(sr: u32, hz: f32, n: usize) -> Vec<f32> {
        // Phase in f64: at 15 kHz the f32 phase loses enough precision to add
        // audible noise to the test tone itself.
        (0..n)
            .map(|i| ((2.0 * std::f64::consts::PI * hz as f64 * i as f64 / sr as f64).sin() * 0.5) as f32)
            .collect()
    }

    /// A 1 kHz tone must survive 44.1k → 48k → 44.1k in place and in level.
    #[test]
    fn round_trip_keeps_the_signal_aligned() {
        let x = sine(44_100, 1000.0, 44_100);
        let y = resample(&resample(&x, 44_100, 48_000), 48_000, 44_100);
        assert_eq!(y.len(), x.len());
        let err: f32 = x[2000..42_000]
            .iter()
            .zip(&y[2000..42_000])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 1e-3, "max error {err}");
    }

    #[test]
    fn matches_the_reference_resampler() {
        let x = sine(22_050, 440.0, 22_050);
        let a = resample(&x, 22_050, 48_000);
        let b = cathar::resample(&x, 22_050, 48_000);
        assert_eq!(a.len(), b.len());
        let err: f32 =
            a[2000..40_000].iter().zip(&b[2000..40_000]).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max);
        assert!(err < 1e-3, "max error {err}");
    }

    #[test]
    fn downsampling_removes_what_would_alias() {
        let x = sine(48_000, 15_000.0, 48_000); // above the 8 kHz Nyquist of 16k
        let y = resample(&x, 48_000, 16_000);
        let peak = y[1000..15_000].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak < 1e-3, "aliased peak {peak}");
    }
}
