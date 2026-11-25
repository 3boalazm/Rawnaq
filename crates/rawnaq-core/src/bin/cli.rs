//! Headless run of the whole pipeline, for testing the core without the UI.
//!
//!   rawnaq-cli <input> <output> [strength 0..1] [protect 0..1] [removed.wav] [hiss 0..1]
//!   rawnaq-cli probe <input> [with-denoiser]
//!   rawnaq-cli bands <input>
//!   rawnaq-cli compare <before> <after>\n//!   rawnaq-cli rescue <original> <denoised elsewhere> <output>

use anyhow::{bail, Result};
use rawnaq_core::*;
use std::path::Path;
use std::time::Instant;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("moss") {
        return moss(&args[2], &args[3], &args[4]);
    }
    if args.get(1).map(String::as_str) == Some("rescue") {
        return rescue(&args[2], &args[3], &args[4]);
    }
    if args.get(1).map(String::as_str) == Some("compare") {
        return compare(&args[2], &args[3]);
    }
    if args.get(1).map(String::as_str) == Some("bands") {
        return bands(&args[2]);
    }
    if args.get(1).map(String::as_str) == Some("probe") {
        return probe(&args[2], args.get(3).is_some());
    }
    if args.len() < 3 {
        bail!("usage: rawnaq-cli <input> <output> [strength 0..1] [protect 0..1] [removed.wav]");
    }
    let strength: f32 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(0.85);
    let protect: f32 = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(0.5);
    let hiss: f32 = args.get(6).map(|s| s.parse()).transpose()?.unwrap_or(0.6);

    let t = Instant::now();
    let original = load(Path::new(&args[1]))?;
    let d = diagnose(&original);
    println!("diagnosis: {}", serde_json::to_string(&d)?);

    let repaired = repair(&original, &RepairSettings::default(), &no_progress);
    let score = detect_voice(&repaired).score();
    let profile = learn_noise(&repaired, &score);
    println!("noise: {}", serde_json::to_string(&profile)?);
    let clean = subtract_noise(&repaired, &profile, &score);
    println!("voice and noise done at {:.1}s", t.elapsed().as_secs_f32());
    let den = denoise_full(&repaired, &no_progress)?;
    println!("denoise done at {:.1}s", t.elapsed().as_secs_f32());

    let mask = voice_mask(&score, repaired.channels[0].len(), repaired.sample_rate);
    let mixed = mix_protected(&repaired, &clean, &den.wet, hiss, strength, protect, &mask);
    let mut q = quality_gate(&repaired, &mixed, &score);
    q.flagged_regions.clear();
    println!("quality: {}", serde_json::to_string(&q)?);

    let loss = tone_loss(&repaired, &mixed, &mask);
    let (output, report) = enhance(&mixed, &EnhanceSettings::default(), loss);
    println!("enhance: {}", serde_json::to_string(&report)?);

    save(&output, Path::new(&args[2]))?;
    if let Some(p) = args.get(5) {
        write_preview_wav(&residual(&original, &mixed), Path::new(p))?;
    }
    println!("wrote {} ({:.1}s of audio in {:.1}s)", args[2], d.duration_sec, t.elapsed().as_secs_f32());
    Ok(())
}

/// Print what each voice detector says, second by second.
///   rawnaq-cli probe <input> [with-denoiser]
fn probe(input: &str, with_denoiser: bool) -> Result<()> {
    let t = Instant::now();
    let audio = load(Path::new(input))?;
    let track = voice::detect_voice(&audio);
    println!("detect_voice: {} frames in {:.1}s", track.len(), t.elapsed().as_secs_f32());
    let lsnr = if with_denoiser { denoise_full(&audio, &no_progress)?.lsnr } else { Vec::new() };
    let share = |v: &[f32], th: f32| v.iter().filter(|x| **x >= th).count() as f32 / v.len().max(1) as f32;
    let score = track.score();
    println!(
        "speech>=0.5: {:.2}  pitch>=0.8: {:.2}  score>=0.6: {:.2}  score>=0.9: {:.2}  score<=0.15: {:.2}",
        share(&track.speech, 0.5),
        share(&track.pitch, 0.8),
        share(&score, 0.6),
        share(&score, 0.9),
        1.0 - share(&score, 0.15)
    );
    let t = Instant::now();
    let profile = learn_noise(&audio, &score);
    let clean = subtract_noise(&audio, &profile, &score);
    println!("noise: {} in {:.1}s", serde_json::to_string(&profile)?, t.elapsed().as_secs_f32());
    let q = quality_gate(&audio, &clean, &score);
    println!("profile engine alone: voice loss {:.2} dB, noise cut {:.1} dB, flagged {:.3}", q.voice_loss_db, q.noise_reduction_db, q.flagged_ratio);
    println!("sec  level  speech pitch score  lsnr");
    for s in 0..track.len() / 100 {
        let r = s * 100..(s + 1) * 100;
        let mean = |v: &[f32]| v[r.clone()].iter().sum::<f32>() / 100.0;
        let l = lsnr
            .first()
            .and_then(|l| l.get(r.clone()))
            .map_or(f32::NAN, |l| l.iter().sum::<f32>() / 100.0);
        println!(
            "{s:4} {:6.1} {:5.2} {:5.2} {:5.2} {l:6.1}",
            mean(&track.level_db),
            mean(&track.speech),
            mean(&track.pitch),
            mean(&score)
        );
    }
    Ok(())
}

/// Level per frequency band, in voice frames and in frames without a voice,
/// for the recording, the profile engine's output and the neural denoiser's.
///   rawnaq-cli bands <input>
fn bands(input: &str) -> Result<()> {
    use realfft::RealFftPlanner;
    const N: usize = 2048;
    const EDGES: [f32; 7] = [0.0, 80.0, 300.0, 1000.0, 3000.0, 8000.0, 1e9];
    let original = load(Path::new(input))?;
    let audio = repair(&original, &RepairSettings::default(), &no_progress);
    let score = detect_voice(&audio).score();
    let profile = learn_noise(&audio, &score);
    let clean = subtract_noise(&audio, &profile, &score);
    let wet = denoise_full(&audio, &no_progress)?.wet;
    let sr = audio.sample_rate as f32;
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N);
    let win: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
    let measure = |x: &AudioData| {
        let (mut buf, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
        // [voice, gap] × band → (power sum, frames)
        let mut acc = [[0f64; 6]; 2];
        let mut count = [0usize; 2];
        let mut valley = [[0f64; 6]; 2];
        let ch = &x.channels[0];
        for start in (0..ch.len().saturating_sub(N)).step_by(N / 2) {
            let f = ((start + N / 2) as f32 / sr * 100.0) as usize;
            let (a, b) = (f.saturating_sub(10), (f + 10).min(score.len() - 1));
            let (lo, hi) = score[a..=b].iter().fold((1f32, 0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            let group = if lo >= 0.6 { 0 } else if hi <= 0.15 { 1 } else { continue };
            buf.iter_mut().zip(&ch[start..]).zip(&win).for_each(|((b, s), w)| *b = s * w);
            let _ = fft.process(&mut buf, &mut spec);
            count[group] += 1;
            // The valleys between the harmonics: the quietest tenth of the
            // bins in each band, which is where noise under a voice shows.
            for (b, v) in valley[group].iter_mut().enumerate() {
                let mut bins: Vec<f32> = spec
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| {
                        let hz = *k as f32 * sr / N as f32;
                        hz >= EDGES[b] && hz < EDGES[b + 1]
                    })
                    .map(|(_, x)| x.norm_sqr())
                    .collect();
                bins.sort_by(|a, b| a.total_cmp(b));
                *v += 10.0 * (bins[bins.len() / 10] as f64).max(1e-20).log10();
            }
            for (k, v) in spec.iter().enumerate() {
                let hz = k as f32 * sr / N as f32;
                let band = EDGES.iter().rposition(|e| hz >= *e).unwrap().min(5);
                acc[group][band] += v.norm_sqr() as f64;
            }
        }
        let db = |g: usize, b: usize| 10.0 * (acc[g][b] / count[g].max(1) as f64 * 8.0 / (3.0 * (N * N) as f64)).max(1e-14).log10();
        let norm = 10.0 * (8.0 / (3.0 * (N * N) as f64)).log10();
        let valleys: Vec<f64> = (0..6).map(|b| valley[0][b] / count[0].max(1) as f64 + norm).collect();
        (count, (0..6).map(|b| (db(0, b), db(1, b))).collect::<Vec<_>>(), valleys)
    };
    println!("bands (Hz): <80 | 80-300 | 300-1k | 1k-3k | 3k-8k | >8k   (dBFS, voice frames / gap frames)");
    for (name, x) in [("repaired", &audio), ("profile ", &clean), ("neural  ", &wet)] {
        let (count, rows, valleys) = measure(x);
        let line: Vec<String> = rows.iter().map(|(v, g)| format!("{v:6.1}/{g:6.1}")).collect();
        println!("{name} frames {:?}: {}", count, line.join(" | "));
        let line: Vec<String> = valleys.iter().map(|v| format!("{v:13.1}")).collect();
        println!("  valleys under the voice (per bin): {}", line.join(" | "));
    }
    println!("noise: {}", serde_json::to_string(&profile)?);

    // How the signal in each bin compares with the measured floor: the
    // 10th, 50th and 90th percentile of power / floor, in dB.
    let n = profile.fft;
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
    let win: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).sqrt()).collect();
    let (mut buf, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
    let mut ratios: [[Vec<f32>; 6]; 2] = Default::default();
    let ch = &audio.channels[0];
    for start in (0..ch.len().saturating_sub(n)).step_by(n) {
        let f = ((start + n / 2) as f32 / sr * 100.0) as usize;
        let (a, b) = (f.saturating_sub(10), (f + 10).min(score.len() - 1));
        let (lo, hi) = score[a..=b].iter().fold((1f32, 0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let group = if lo >= 0.6 { 0 } else if hi <= 0.15 { 1 } else { continue };
        buf.iter_mut().zip(&ch[start..]).zip(&win).for_each(|((b, s), w)| *b = s * w);
        let _ = fft.process(&mut buf, &mut spec);
        let floor = if group == 0 { &profile.voiced_power } else { &profile.power };
        for (k, v) in spec.iter().enumerate().step_by(3) {
            let hz = k as f32 * sr / n as f32;
            let band = EDGES.iter().rposition(|e| hz >= *e).unwrap().min(5);
            ratios[group][band].push(10.0 * (v.norm_sqr() / floor[k].max(1e-20)).max(1e-12).log10());
        }
    }
    for (g, name) in ["voice", "gaps "].iter().enumerate() {
        let line: Vec<String> = ratios[g]
            .iter_mut()
            .map(|r| {
                r.sort_by(|a, b| a.total_cmp(b));
                let at = |q: usize| r.get(r.len() * q / 100).copied().unwrap_or(f32::NAN);
                format!("{:5.1} {:5.1} {:5.1}", at(10), at(50), at(90))
            })
            .collect();
        println!("power/floor dB p10 p50 p90, {name}: {}", line.join(" | "));
    }
    Ok(())
}

/// What a processed file lost against its source: in the voice, in the pauses,
/// and band by band. Both files must have the same sample rate and length.
///   rawnaq-cli compare <before> <after>
fn compare(before: &str, after: &str) -> Result<()> {
    let a = load(Path::new(before))?;
    let mut b = load(Path::new(after))?;
    for c in &mut b.channels {
        c.resize(a.channels[0].len(), 0.0);
    }
    let score = detect_voice(&a).score();
    let mut q = quality_gate(&a, &b, &score);
    q.flagged_regions.clear();
    println!("quality: {}", serde_json::to_string(&q)?);
    let mask = voice_mask(&score, a.channels[0].len(), a.sample_rate);
    let (low, high) = tone_loss(&a, &b, &mask);
    println!("tone in the voice, relative to its overall change: low {low:+.2} dB, high {high:+.2} dB");
    // Level of the pauses and of the voice, frame by frame.
    let hop = a.sample_rate as usize / 100;
    let level = |x: &AudioData, voiced: bool| {
        let (mut e, mut n) = (0f64, 0usize);
        for (f, v) in score.iter().enumerate() {
            let (lo, hi) = (f.saturating_sub(10), (f + 10).min(score.len() - 1));
            let near = &score[lo..=hi];
            let keep = if voiced { near.iter().all(|s| *s >= 0.6) } else { near.iter().all(|s| *s <= 0.15) };
            if !keep || (f + 1) * hop > x.channels[0].len() {
                continue;
            }
            e += x.channels[0][f * hop..(f + 1) * hop].iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
            n += hop;
            let _ = v;
        }
        10.0 * (e / n.max(1) as f64).max(1e-14).log10()
    };
    println!(
        "voice {:.1} -> {:.1} dBFS, pauses {:.1} -> {:.1} dBFS",
        level(&a, true), level(&b, true), level(&a, false), level(&b, false)
    );
    Ok(())
}

/// Take another denoiser's output and give back the voice it cut: wherever it
/// dropped a detected voice, use the profile engine's output instead.
///   rawnaq-cli rescue <original> <denoised elsewhere> <output>
fn rescue(original: &str, denoised: &str, output: &str) -> Result<()> {
    let dry = load(Path::new(original))?;
    let mut wet = load(Path::new(denoised))?;
    let n = dry.channels[0].len();
    for c in &mut wet.channels {
        c.resize(n, 0.0);
    }
    let score = detect_voice(&dry).score();
    let profile = learn_noise(&dry, &score);
    let clean = subtract_noise(&dry, &profile, &score);
    let cut = voice_cut(&dry, &wet, &score);
    let share = cut.iter().filter(|c| **c > 0.5).count() as f32 / cut.len().max(1) as f32;
    println!("the denoiser cut the voice in {:.0}% of the recording", share * 100.0);
    let guard = voice_mask(&cut, n, dry.sample_rate);
    let mixed = mix_protected(&dry, &clean, &wet, 1.0, 1.0, 1.0, &guard);
    let mut q = quality_gate(&dry, &mixed, &score);
    q.flagged_regions.clear();
    println!("quality: {}", serde_json::to_string(&q)?);
    write_preview_wav(&mixed, Path::new(output))?;
    Ok(())
}

/// Run MossFormer2 alone, to check this port against the reference.
///   rawnaq-cli moss <models dir> <input> <output>
fn moss(models: &str, input: &str, output: &str) -> Result<()> {
    let audio = load(Path::new(input))?;
    let t = Instant::now();
    let mut engine = mossformer::MossFormer::load(Path::new(models))?;
    println!("model loaded in {:.1}s", t.elapsed().as_secs_f32());
    let t = Instant::now();
    let out = engine.denoise(&audio, &no_progress)?;
    let secs = audio.channels[0].len() as f32 / audio.sample_rate as f32;
    println!("{secs:.1}s of audio in {:.1}s", t.elapsed().as_secs_f32());
    write_preview_wav(&out, Path::new(output))?;
    Ok(())
}
