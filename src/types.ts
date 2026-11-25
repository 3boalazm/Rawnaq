// Mirrors the structs serialised by src-tauri/src/lib.rs and rawnaq-core.

export interface Peaks {
  min: number[];
  max: number[];
}

export interface Diagnosis {
  sample_rate: number;
  channels: number;
  duration_sec: number;
  peak_dbfs: number;
  rms_dbfs: number;
  loudness_lufs: number;
  noise_floor_dbfs: number;
  snr_db: number;
  clipped_samples: number;
  clipped_runs: number;
  mains_hum_hz: number | null;
  dropouts: number;
  issues: string[];
}

export interface Loaded {
  file_name: string;
  diagnosis: Diagnosis;
  peaks: Peaks;
  preview: string;
}

export type Verdict = "ok" | "caution" | "warn" | "unknown";

export interface QualityReport {
  verdict: Verdict;
  voice_loss_db: number;
  flagged_ratio: number;
  noise_reduction_db: number;
  voice_ratio: number;
  flagged_regions: [number, number][];
}

export interface Processed {
  peaks: Peaks;
  quality: QualityReport | null;
  preview: string;
  removed_preview: string;
  removed_peak_dbfs: number;
  removed_peaks: Peaks;
  voice_regions: [number, number][];
  /** Share of the recording where a human voice was detected, 0..1. */
  voice_share: number;
  noise: NoiseProfile | null;
  enhance: EnhanceReport;
}

/** The steady noise measured in the recording, levels in dBFS. */
export interface NoiseProfile {
  level_dbfs: number;
  /** Noise level while a voice is present; higher when noise rides on the voice. */
  voiced_dbfs: number;
  low_dbfs: number;
  mid_dbfs: number;
  high_dbfs: number;
  kind: "low" | "mid" | "high";
}

export interface EnhanceReport {
  low_loss_db: number;
  high_loss_db: number;
  low_gain_db: number;
  high_gain_db: number;
  gain_db: number;
  limited_db: number;
}

export interface EnhanceSettings {
  tone_restore: boolean;
  warmth_db: number;
  clarity_db: number;
  loudness: boolean;
  target_lufs: number;
  master_db: number;
  stereo: boolean;
}

export interface RepairSettings {
  fill_dropouts: boolean;
  declick: boolean;
  declick_threshold: number;
  dehum: boolean;
  declip: boolean;
}

export interface Settings {
  repair: RepairSettings;
  denoise: boolean;
  engine: Engine;
  /** Share of the measured noise floor removed everywhere, 0..1. */
  hiss: number;
  strength: number;
  protect: number;
  enhance: EnhanceSettings;
}

/** The deep denoiser that runs on top of the noise-profile engine. */
export type Engine = "deepfilter" | "mossformer";

export type Mode = "original" | "restored" | "removed";
