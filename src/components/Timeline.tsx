import { useEffect, useRef } from "react";
import { formatTime } from "../lib/format";
import type { Mode, Peaks } from "../types";

export interface Track {
  id: Mode;
  label: string;
  hint: string;
  peaks: Peaks | null;
  /** CSS custom property holding the track's colour. */
  color: string;
}

interface Props {
  tracks: Track[];
  active: Mode;
  duration: number;
  time: number;
  voice: [number, number][];
  flagged: [number, number][];
  onSelect: (m: Mode) => void;
  onSeek: (t: number) => void;
}

const RULER_STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];

function Bars({ peaks, color }: { peaks: Peaks; color: string }) {
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const draw = () => {
      const dpr = window.devicePixelRatio || 1;
      const w = Math.max(1, Math.round(c.clientWidth * dpr));
      const h = Math.max(1, Math.round(c.clientHeight * dpr));
      if (c.width !== w || c.height !== h) {
        c.width = w;
        c.height = h;
      }
      const ctx = c.getContext("2d")!;
      ctx.clearRect(0, 0, w, h);
      ctx.fillStyle = getComputedStyle(c).getPropertyValue(color).trim();
      const n = peaks.min.length;
      const pitch = 3 * dpr;
      const bar = 1.6 * dpr;
      const mid = h / 2;
      for (let x = 0; x + bar <= w; x += pitch) {
        const a = Math.floor((x * n) / w);
        const b = Math.max(a + 1, Math.floor(((x + pitch) * n) / w));
        let amp = 0;
        for (let i = a; i < b && i < n; i++) amp = Math.max(amp, peaks.max[i], -peaks.min[i]);
        const half = Math.max(dpr * 0.75, Math.min(1, amp) * mid * 0.9);
        ctx.beginPath();
        ctx.roundRect(x, mid - half, bar, half * 2, bar / 2);
        ctx.fill();
      }
    };
    draw();
    const ro = new ResizeObserver(draw);
    ro.observe(c);
    return () => ro.disconnect();
  }, [peaks, color]);

  return <canvas ref={canvas} />;
}

/** Tracks stacked on one time axis. Click a track's name to listen to it. */
export default function Timeline(p: Props) {
  const step = RULER_STEPS.find((s) => p.duration / s <= 12) ?? 3600;
  const marks = Array.from({ length: Math.floor(p.duration / step) + 1 }, (_, i) => i * step);
  const pct = (t: number) => `${(t / p.duration) * 100}%`;

  const seekFrom = (e: React.MouseEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    p.onSeek(Math.min(p.duration, Math.max(0, ((e.clientX - r.left) / r.width) * p.duration)));
  };

  return (
    <div className="timeline" dir="ltr">
      <div className="tl-names">
        <div className="tl-corner">المسارات</div>
        {p.tracks.map((t) => (
          <button
            key={t.id}
            className={`tl-name ${p.active === t.id ? "on" : ""}`}
            style={{ ["--track" as string]: `var(${t.color})` }}
            disabled={!t.peaks}
            aria-pressed={p.active === t.id}
            onClick={() => p.onSelect(t.id)}
            title={t.hint}
          >
            <i aria-hidden="true" />
            <span>{t.label}</span>
          </button>
        ))}
        <div className="tl-name static">
          <i aria-hidden="true" style={{ background: "var(--ok)" }} />
          <span>الصوت البشري</span>
        </div>
      </div>

      <div
        className="tl-lanes"
        role="slider"
        tabIndex={0}
        aria-label="موضع التشغيل"
        aria-valuemin={0}
        aria-valuemax={Math.round(p.duration)}
        aria-valuenow={Math.round(p.time)}
        aria-valuetext={formatTime(p.time)}
        onClick={seekFrom}
        onKeyDown={(e) => {
          if (e.key === "ArrowRight") p.onSeek(Math.min(p.duration, p.time + 5));
          if (e.key === "ArrowLeft") p.onSeek(Math.max(0, p.time - 5));
        }}
      >
        <div className="tl-ruler">
          {marks.map((m) => (
            <span key={m} style={{ left: pct(m) }}>
              {formatTime(m)}
            </span>
          ))}
        </div>

        {p.tracks.map((t) => (
          <div
            key={t.id}
            className={`tl-lane ${p.active === t.id ? "on" : ""} ${t.peaks ? "" : "empty"}`}
            style={{ ["--track" as string]: `var(${t.color})` }}
          >
            {t.peaks ? <Bars peaks={t.peaks} color={t.color} /> : <span>يظهر بعد الترميم</span>}
            {t.id === "restored" &&
              p.flagged.map(([s, e], i) => (
                <b key={i} className="tl-flag" style={{ left: pct(s), width: `max(3px, ${pct(e - s)})` }} />
              ))}
          </div>
        ))}

        <div className="tl-lane clips">
          {p.voice.length === 0 && <span>يُرصد بعد الترميم</span>}
          {p.voice.map(([s, e], i) => (
            <b key={i} style={{ left: pct(s), width: pct(e - s) }} />
          ))}
        </div>

        <div className="tl-playhead" style={{ left: pct(p.time) }} />
      </div>
    </div>
  );
}
