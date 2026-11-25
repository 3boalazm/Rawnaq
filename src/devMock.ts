// Development only: lets the interface run in an ordinary browser (`npm run dev`)
// with canned data, so layout work does not need the native shell. Never
// loaded inside Tauri or in a production build (see main.tsx).
import { mockConvertFileSrc, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { Loaded, Processed } from "./types";

function fakePeaks(scale: number) {
  const n = 1600;
  const max = Array.from({ length: n }, (_, i) => {
    const phrase = Math.max(0, Math.sin(i / 55)) ** 0.6;
    return (0.08 + phrase * (0.5 + 0.4 * Math.abs(Math.sin(i * 1.7)))) * scale;
  });
  return { max, min: max.map((v) => -v) };
}

const loaded: Loaded = {
  file_name: "سورة الرحمن - تسجيل قديم 1962.mp3",
  diagnosis: {
    sample_rate: 44100,
    channels: 1,
    duration_sec: 754,
    peak_dbfs: -1.9,
    rms_dbfs: -23.9,
    loudness_lufs: -24.2,
    noise_floor_dbfs: -33.6,
    snr_db: 9.7,
    clipped_samples: 0,
    clipped_runs: 0,
    mains_hum_hz: 50,
    dropouts: 2,
    issues: ["noise", "hum", "dropouts"],
  },
  peaks: fakePeaks(1),
  preview: "original.wav",
};

const processed: Processed = {
  peaks: fakePeaks(0.82),
  quality: {
    verdict: "caution",
    voice_loss_db: 0.9,
    flagged_ratio: 0.031,
    noise_reduction_db: 15.7,
    voice_ratio: 0.41,
    flagged_regions: [
      [120, 124],
      [402, 409],
    ],
  },
  preview: "restored.wav",
  removed_preview: "removed.wav",
  removed_peak_dbfs: -18,
  removed_peaks: fakePeaks(0.14),
  voice_share: 0.62,
  noise: { level_dbfs: -48, voiced_dbfs: -44, low_dbfs: -55, mid_dbfs: -52, high_dbfs: -50, kind: "high" },
  voice_regions: [
    [10, 150],
    [180, 320],
    [350, 500],
    [530, 740],
  ],
  enhance: {
    low_loss_db: 2.4,
    high_loss_db: 1.1,
    low_gain_db: 2.4,
    high_gain_db: 1.1,
    gain_db: 5.6,
    limited_db: 0.8,
  },
};

export function installDevMock() {
  mockWindows("main");
  mockConvertFileSrc("windows");
  mockIPC(
    (cmd) => {
      if (cmd === "load_audio") return loaded;
      if (cmd === "process") return processed;
      if (cmd === "engines") return { mossformer: true };
      if (cmd === "plugin:dialog|open") return "C:/demo/recitation.mp3";
      return null;
    },
    { shouldMockEvents: true },
  );
}
