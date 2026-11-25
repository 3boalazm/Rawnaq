//! Tauri shell around `rawnaq-core`: holds the session, caches each stage,
//! and writes preview files the webview plays through the asset protocol.

use rawnaq_core::mossformer::{find_models, MossFormer};
use rawnaq_core::{
    denoise_full, detect_voice, diagnose, enhance, learn_noise, load, mix_protected, peaks,
    quality_gate, repair, residual, save, subtract_noise, tone_loss, voice_cut, voice_mask,
    voice_regions,
    write_preview_wav, AudioData, Denoised, Diagnosis, EnhanceReport, EnhanceSettings, NoiseProfile,
    Peaks, QualityReport, RepairSettings,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

const WAVE_BUCKETS: usize = 1600;

#[derive(Default)]
struct Session {
    original: Option<Arc<AudioData>>,
    /// Stage cache: the repair result and the settings that produced it.
    repaired: Option<(RepairSettings, Arc<AudioData>)>,
    /// Stage cache: what the detectors found in `repaired`.
    analysis: Option<Arc<Analysis>>,
    /// Stage cache: the full-strength denoise of `repaired`, per engine.
    denoised: Vec<(Engine, Arc<Denoised>)>,
    output: Option<Arc<AudioData>>,
}

/// The voice and noise detectors' findings for one repaired recording.
struct Analysis {
    /// Voice score per 10 ms frame, 0..1.
    score: Vec<f32>,
    /// The same, one value per sample.
    mask: Vec<f32>,
    noise: NoiseProfile,
    /// The recording with its measured noise floor turned down, full strength.
    clean: AudioData,
}

/// The deep denoiser that runs on top of the noise-profile engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Engine {
    /// DeepFilterNet 3: fast, built in. Held back wherever a voice is.
    #[default]
    DeepFilter,
    /// MossFormer2: slower, needs its model. Cleans under the voice too; the
    /// stretches of voice it cuts are handed back to the profile engine.
    MossFormer,
}

#[derive(Default)]
struct AppState {
    session: Mutex<Session>,
    /// Loaded on first use and kept: loading the model takes seconds.
    mossformer: Arc<Mutex<Option<MossFormer>>>,
    preview_id: AtomicU64,
}

#[derive(Debug, Clone, Deserialize)]
struct Settings {
    repair: RepairSettings,
    denoise: bool,
    #[serde(default)]
    engine: Engine,
    /// 0..1, how much of the measured noise floor is removed, everywhere.
    #[serde(default = "default_hiss")]
    hiss: f32,
    /// 0..1, the share of the neural denoiser in the output.
    strength: f32,
    /// 0..1, how much of that strength is held back where the voice is.
    protect: f32,
    enhance: EnhanceSettings,
}

fn default_hiss() -> f32 {
    0.6
}

#[derive(Serialize)]
struct Loaded {
    file_name: String,
    diagnosis: Diagnosis,
    peaks: Peaks,
    preview: String,
}

#[derive(Serialize)]
struct Processed {
    peaks: Peaks,
    quality: Option<QualityReport>,
    preview: String,
    removed_preview: String,
    /// Peak level of the removed signal, dBFS: how much was taken out at most.
    removed_peak_dbfs: f32,
    /// Waveform of the removed signal, for its own track.
    removed_peaks: Peaks,
    /// Spans (seconds) where a human voice was detected.
    voice_regions: Vec<[f32; 2]>,
    /// Share of the recording where a voice was detected, 0..1.
    voice_share: f32,
    /// The steady noise measured in the recording.
    noise: Option<NoiseProfile>,
    enhance: EnhanceReport,
}

#[derive(Clone, Serialize)]
struct ProgressEvent {
    stage: String,
    fraction: f32,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{e:#}")
}

fn preview_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_cache_dir().map_err(err)?.join("preview");
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir)
}

/// Write a preview WAV under a name never used before, so the webview cannot
/// play a cached file from an earlier run or an earlier session.
///
/// The previous file of the same kind is kept: the player may still be reading
/// it while the new one is being written. Anything older is deleted.
fn write_preview(
    app: &AppHandle,
    state: &AppState,
    kind: &str,
    audio: &AudioData,
) -> Result<String, String> {
    let dir = preview_dir(app)?;
    // Milliseconds since the epoch plus a counter: unique and sortable by age.
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = state.preview_id.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{kind}-{ms:015}-{n}.wav"));
    write_preview_wav(audio, &path).map_err(err)?;

    let prefix = format!("{kind}-");
    let mut same_kind: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    same_kind.retain(|p| file_name(p).starts_with(&prefix));
    same_kind.sort();
    let stale = same_kind.len().saturating_sub(2);
    for old in &same_kind[..stale] {
        let _ = std::fs::remove_file(old);
    }
    // Hand the webview the file's real location. Windows can redirect writes
    // under AppData for a process started from a packaged app; the asset
    // protocol compares real paths and refuses (403) a file reached through
    // the redirected name.
    let real = std::fs::canonicalize(&path).map_err(err)?;
    let real = match real.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) => PathBuf::from(plain),
        None => real,
    };
    if let Some(parent) = real.parent() {
        app.asset_protocol_scope().allow_directory(parent, false).map_err(err)?;
    }
    Ok(real.to_string_lossy().into_owned())
}

fn peak_dbfs(audio: &AudioData) -> f32 {
    let peak = audio.channels.iter().flatten().fold(0f32, |m, s| m.max(s.abs()));
    (20.0 * peak.max(1e-6).log10()).max(-120.0)
}

#[tauri::command]
async fn load_audio(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Loaded, String> {
    let p = PathBuf::from(&path);
    let (audio, diagnosis, pk) = tauri::async_runtime::spawn_blocking({
        let p = p.clone();
        move || -> Result<_, String> {
            let audio = load(&p).map_err(err)?;
            let d = diagnose(&audio);
            let pk = peaks(&audio, WAVE_BUCKETS);
            Ok((audio, d, pk))
        }
    })
    .await
    .map_err(err)??;

    let preview = write_preview(&app, &state, "original", &audio)?;
    *state.session.lock().unwrap() =
        Session { original: Some(Arc::new(audio)), ..Default::default() };

    Ok(Loaded {
        file_name: file_name(&p),
        diagnosis,
        peaks: pk,
        preview,
    })
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Run the pipeline, reusing every cached stage whose inputs did not change:
/// a new strength only re-mixes, new repair settings redo repair and denoise.
#[tauri::command]
async fn process(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Processed, String> {
    let engine = settings.engine;
    let (original, cached_repair, cached_analysis, cached_denoise) = {
        let s = state.session.lock().unwrap();
        let original = s.original.clone().ok_or("no recording is loaded")?;
        let denoised = s.denoised.iter().find(|(e, _)| *e == engine).map(|(_, d)| d.clone());
        (original, s.repaired.clone(), s.analysis.clone(), denoised)
    };
    let models = models_dir(&app);
    let moss = state.mossformer.clone();

    let progress_app = app.clone();
    let work = tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
        let progress = move |stage: &str, fraction: f32| {
            let _ = progress_app
                .emit("progress", ProgressEvent { stage: stage.to_string(), fraction });
        };

        let (repaired, repair_reused) = match cached_repair {
            Some((s, a)) if s == settings.repair => (a, true),
            _ if settings.repair.any() => {
                progress("repair", 0.0);
                (Arc::new(repair(&original, &settings.repair, &progress)), false)
            }
            _ => (original.clone(), false),
        };

        // The detectors look at the repaired recording, before any denoising.
        let analysis = if !settings.denoise {
            None
        } else {
            match cached_analysis {
                Some(a) if repair_reused => Some(a),
                _ => {
                    progress("detect", 0.0);
                    let score = detect_voice(&repaired).score();
                    let mask = voice_mask(&score, repaired.channels[0].len(), repaired.sample_rate);
                    progress("detect", 0.5);
                    let noise = learn_noise(&repaired, &score);
                    let clean = subtract_noise(&repaired, &noise, &score);
                    Some(Arc::new(Analysis { score, mask, noise, clean }))
                }
            }
        };

        let denoised = if !settings.denoise {
            None
        } else {
            match cached_denoise {
                Some(d) if repair_reused => Some(d),
                _ => {
                    progress("denoise", 0.0);
                    Some(Arc::new(match engine {
                        Engine::DeepFilter => denoise_full(&repaired, &progress).map_err(err)?,
                        Engine::MossFormer => {
                            let dir = models.as_deref().ok_or("the MossFormer2 model is not installed")?;
                            let mut slot = moss.lock().unwrap();
                            if slot.is_none() {
                                *slot = Some(MossFormer::load(dir).map_err(err)?);
                            }
                            let wet = slot.as_mut().unwrap().denoise(&repaired, &progress).map_err(err)?;
                            Denoised { wet, lsnr: Vec::new() }
                        }
                    }))
                }
            }
        };

        progress("finish", 0.0);
        // The gate and the "removed" signal describe the denoisers alone, so
        // they are taken before tone and loudness change the level.
        let (mixed, quality, loss, regions, share) = match (&denoised, &analysis) {
            (Some(d), Some(a)) => {
                // What "protect" guards. DeepFilterNet is kept off every
                // voice. MossFormer2 is trusted under the voice, except where
                // it cut a detected voice out.
                let cut;
                let guard = match engine {
                    Engine::DeepFilter => &a.mask,
                    Engine::MossFormer => {
                        let frames = voice_cut(&repaired, &d.wet, &a.score);
                        cut = voice_mask(&frames, repaired.channels[0].len(), repaired.sample_rate);
                        &cut
                    }
                };
                let mixed = mix_protected(
                    &repaired,
                    &a.clean,
                    &d.wet,
                    settings.hiss,
                    settings.strength,
                    settings.protect,
                    guard,
                );
                let q = quality_gate(&repaired, &mixed, &a.score);
                let loss = tone_loss(&repaired, &mixed, &a.mask);
                let regions = voice_regions(&a.mask, repaired.sample_rate);
                let voiced = a.score.iter().filter(|v| **v > 0.5).count();
                let share = voiced as f32 / a.score.len().max(1) as f32;
                (Arc::new(mixed), Some(q), loss, regions, share)
            }
            _ => (repaired.clone(), None, (0.0, 0.0), Vec::new(), 0.0),
        };
        progress("finish", 0.4);
        let removed = residual(&original, &mixed);
        let (output, report) = enhance(&mixed, &settings.enhance, loss);
        let output = Arc::new(output);
        let pk = peaks(&output, WAVE_BUCKETS);
        Ok((repaired, analysis, denoised, output, quality, removed, pk, regions, share, report))
    });
    let (repaired, analysis, denoised, output, quality, removed, pk, regions, share, report) =
        work.await.map_err(err)??;

    let preview = write_preview(&app, &state, "restored", &output)?;
    let removed_preview = write_preview(&app, &state, "removed", &removed)?;
    let removed_peak_dbfs = peak_dbfs(&removed);
    let removed_peaks = peaks(&removed, WAVE_BUCKETS);

    let mut s = state.session.lock().unwrap();
    // Results cached for other repair settings describe a different signal.
    if !matches!(&s.repaired, Some((r, _)) if *r == settings.repair) {
        s.denoised.clear();
        s.analysis = None;
    }
    s.repaired = Some((settings.repair, repaired));
    // Keep the caches when denoising is merely switched off.
    let noise = analysis.as_ref().map(|a| a.noise.clone());
    if let Some(d) = denoised {
        s.denoised.retain(|(e, _)| *e != engine);
        s.denoised.push((engine, d));
        s.analysis = analysis;
    }
    s.output = Some(output);

    Ok(Processed {
        peaks: pk,
        quality,
        preview,
        removed_preview,
        removed_peak_dbfs,
        removed_peaks,
        voice_regions: regions,
        voice_share: share,
        noise,
        enhance: report,
    })
}

/// Where the optional models live: next to the program, in the app's data
/// folder, or, in a development build, in the project folder.
fn models_dir(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join("models"));
    }
    if let Ok(dir) = app.path().app_data_dir() {
        candidates.push(dir.join("models"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("models"));
    find_models(&candidates)
}

#[derive(Serialize)]
struct Engines {
    /// Whether the MossFormer2 model and ONNX Runtime were found.
    mossformer: bool,
}

#[tauri::command]
fn engines(app: AppHandle) -> Engines {
    Engines { mossformer: models_dir(&app).is_some() }
}

#[tauri::command]
async fn export_audio(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let output =
        state.session.lock().unwrap().output.clone().ok_or("nothing has been restored yet")?;
    tauri::async_runtime::spawn_blocking(move || save(&output, Path::new(&path)).map_err(err))
        .await
        .map_err(err)?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![load_audio, process, export_audio, engines])
        .run(tauri::generate_context!())
        .expect("error while running Rawnaq");
}
