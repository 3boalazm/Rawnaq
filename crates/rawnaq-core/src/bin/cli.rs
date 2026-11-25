//! Headless run of the whole pipeline, for testing the core without the UI.
//!
//!   rawnaq-cli <input> <output> [strength 0..1] [protect 0..1] [removed.wav] [hiss 0..1]
//!   rawnaq-cli probe <input> [with-denoiser]

use anyhow::{bail, Result};
use rawnaq_core::*;
use std::path::Path;
use std::time::Instant;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
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
    let profile = learn_noise(&repaired);
    println!("noise: {}", serde_json::to_string(&profile)?);
    let clean = subtract_noise(&repaired, &profile);
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
    let profile = learn_noise(&audio);
    let clean = subtract_noise(&audio, &profile);
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
