import { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { AUDIO_EXTENSIONS, DEFAULT_SETTINGS } from "../lib/format";
import type { Loaded, Mode, Processed, Settings } from "../types";

export interface Busy {
  stage: string;
  fraction: number;
  /** When it started, ms since the epoch, for the time remaining. */
  startedAt: number;
}

/**
 * Everything the interface knows about the open recording: what is loaded,
 * the settings, the last result, playback, and the calls into the backend.
 */
export function useSession() {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [processed, setProcessed] = useState<Processed | null>(null);
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [applied, setApplied] = useState<Settings | null>(null);
  const [busy, setBusy] = useState<Busy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [mode, setMode] = useState<Mode>("original");
  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  // Listening level and mute: they change what you hear, never the result.
  const [volume, setVolumeState] = useState(1);
  const [muted, setMuted] = useState(false);
  // A light re-run (mix, master) in flight; unlike `busy` it locks nothing.
  const [syncing, setSyncing] = useState(false);
  // Whether the backend holds a denoise result for the applied repair settings.
  const [denoiseCached, setDenoiseCached] = useState(false);
  // Whether the optional MossFormer2 model is installed.
  const [deepAvailable, setDeepAvailable] = useState(false);
  const audio = useRef<HTMLAudioElement>(null);
  const resume = useRef<{ time: number; play: boolean } | null>(null);

  const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
  const changed = applied !== null && !same(applied, settings);
  // "Live" changes reuse the cached repair and denoise stages, so they are
  // applied on their own. Anything that redoes those stages waits for the button.
  const live =
    changed &&
    same(applied!.repair, settings.repair) &&
    settings.engine === applied!.engine &&
    (settings.denoise === applied!.denoise || !settings.denoise || denoiseCached);
  const dirty = changed && !live;

  const source =
    mode === "original" || !processed
      ? loaded?.preview
      : mode === "restored"
        ? processed.preview
        : processed.removed_preview;

  // Swap the playing file but keep the position, so switching tracks is instant.
  useEffect(() => {
    const el = audio.current;
    if (!el || !source) return;
    resume.current = { time: el.currentTime || 0, play: !el.paused };
    el.src = convertFileSrc(source);
    el.load();
  }, [source]);

  // Prefer the deep engine when its model is installed.
  useEffect(() => {
    invoke<{ mossformer: boolean }>("engines")
      .then((e) => {
        setDeepAvailable(e.mossformer);
        if (e.mossformer) setSettings((s) => ({ ...s, engine: "mossformer" }));
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    const un = listen<{ stage: string; fraction: number }>("progress", (e) =>
      setBusy((b) => (b ? { ...e.payload, startedAt: b.startedAt } : b)),
    );
    return () => {
      un.then((f) => f());
    };
  }, []);

  const loadPath = useCallback(async (path: string) => {
    setError(null);
    setNotice(null);
    setBusy({ stage: "load", fraction: 0, startedAt: Date.now() });
    if (audio.current) {
      // A new recording starts from the beginning, not where the last one was.
      audio.current.pause();
      audio.current.currentTime = 0;
    }
    try {
      const result = await invoke<Loaded>("load_audio", { path });
      setLoaded(result);
      setProcessed(null);
      setApplied(null);
      setDenoiseCached(false);
      setMode("original");
      setTime(0);
      // Offer the repairs the diagnosis calls for.
      setSettings((prev) => ({
        ...DEFAULT_SETTINGS,
        engine: prev.engine,
        repair: { ...DEFAULT_SETTINGS.repair, declip: result.diagnosis.issues.includes("clipping") },
      }));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, []);

  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "drop" && e.payload.paths.length > 0) loadPath(e.payload.paths[0]);
    });
    return () => {
      un.then((f) => f());
    };
  }, [loadPath]);

  const pickFile = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "تسجيل صوتي", extensions: AUDIO_EXTENSIONS }],
    });
    if (typeof path === "string") loadPath(path);
  };

  const run = async () => {
    const s = settings;
    const first = applied === null;
    setError(null);
    setNotice(null);
    setBusy({ stage: "repair", fraction: 0, startedAt: Date.now() });
    try {
      const result = await invoke<Processed>("process", { settings: s });
      setProcessed(result);
      setDenoiseCached(
        s.denoise || (denoiseCached && same(applied?.repair, s.repair) && applied?.engine === s.engine),
      );
      setApplied(s);
      // Only the first run moves you to the result; later you stay on your track.
      if (first) setMode("restored");
    } catch (e) {
      // Cancelling is a choice, not a failure.
      if (!String(e).includes("cancelled")) setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  /** Stop the restoration in progress; the previous result, if any, stays. */
  const cancel = () => {
    invoke("cancel_process").catch(() => {});
  };

  // Apply live changes shortly after the last one, so turning a knob settles
  // first. If more changes arrive while one is in flight, this runs again.
  useEffect(() => {
    if (!live || busy || syncing) return;
    const s = settings;
    const timer = setTimeout(async () => {
      setSyncing(true);
      try {
        const result = await invoke<Processed>("process", { settings: s });
        setProcessed(result);
        if (s.denoise) setDenoiseCached(true);
        setApplied(s);
        setError(null);
      } catch (e) {
        setError(String(e));
        setApplied(s); // do not retry the same failing settings in a loop
      } finally {
        setSyncing(false);
      }
    }, 280);
    return () => clearTimeout(timer);
  }, [live, busy, syncing, settings]);

  const exportFile = async () => {
    if (!loaded) return;
    const stem = loaded.file_name.replace(/\.[^.]+$/, "");
    const path = await save({
      defaultPath: `${stem}-rawnaq.wav`,
      filters: [
        { name: "WAV (32-bit float)", extensions: ["wav"] },
        { name: "FLAC (24-bit)", extensions: ["flac"] },
      ],
    });
    if (!path) return;
    setError(null);
    try {
      await invoke("export_audio", { path });
      setNotice(`تم الحفظ: ${path}`);
    } catch (e) {
      setError(String(e));
    }
  };

  const togglePlay = () => {
    const el = audio.current;
    if (!el || !source) return;
    if (el.paused) el.play();
    else el.pause();
  };

  useEffect(() => {
    if (!audio.current) return;
    audio.current.volume = volume;
    audio.current.muted = muted;
  }, [volume, muted]);

  const setVolume = (v: number) => {
    setVolumeState(Math.round(Math.max(0, Math.min(1, v)) * 10) / 10);
    setMuted(false);
  };
  const toggleMute = () => setMuted((m) => !m);

  const seek = (t: number) => {
    if (audio.current) audio.current.currentTime = t;
    setTime(t);
  };

  const patchEnhance = (patch: Partial<Settings["enhance"]>) =>
    setSettings((s) => ({ ...s, enhance: { ...s.enhance, ...patch } }));

  const patchRepair = (patch: Partial<Settings["repair"]>) =>
    setSettings((s) => ({ ...s, repair: { ...s.repair, ...patch } }));

  const patch = (p: Partial<Settings>) => setSettings((s) => ({ ...s, ...p }));

  /** Props for the single hidden <audio> element that does the playback. */
  const audioProps = {
    ref: audio,
    onLoadedMetadata: (e: React.SyntheticEvent<HTMLAudioElement>) => {
      const r = resume.current;
      if (!r) return;
      resume.current = null;
      e.currentTarget.currentTime = r.time;
      if (r.play) e.currentTarget.play();
    },
    onTimeUpdate: (e: React.SyntheticEvent<HTMLAudioElement>) => {
      // While a new file is loading the element reports 0; keep the old position.
      if (!resume.current) setTime(e.currentTarget.currentTime);
    },
    onPlay: () => setPlaying(true),
    onError: () => {
      // A file that fails to load must not leave the clock frozen.
      resume.current = null;
      setPlaying(false);
    },
    onPause: () => {
      if (!resume.current) setPlaying(false);
    },
    onEnded: () => setPlaying(false),
  };

  return {
    loaded, processed, settings, applied, dirty, busy, syncing, error, notice, deepAvailable,
    mode, setMode, playing, time, volume, muted, setVolume, toggleMute,
    pickFile, run, cancel, exportFile, togglePlay, seek,
    patch, patchEnhance, patchRepair, audioProps,
  };
}

export type Session = ReturnType<typeof useSession>;
