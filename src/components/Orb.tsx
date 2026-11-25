import { useEffect, useRef } from "react";

interface Props {
  /** Signal level at the playhead, 0..1. The sphere swells and ripples with it. */
  level: number;
  /** While true the sphere follows `level`; otherwise it breathes slowly. */
  active: boolean;
}

/** A sphere drawn in dots that moves with the recording. */
export default function Orb({ level, active }: Props) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const target = useRef({ level, active });
  target.current = { level, active };

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const ctx = c.getContext("2d")!;
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    let energy = 0;
    let raf = 0;

    const draw = (ms: number) => {
      const dpr = window.devicePixelRatio || 1;
      const size = c.clientWidth;
      if (c.width !== Math.round(size * dpr)) {
        c.width = c.height = Math.round(size * dpr);
      }
      const t = ms / 1000;
      const { level, active } = target.current;
      const want = active ? Math.min(1, level * 1.6) : 0.12 + 0.06 * Math.sin(t * 1.2);
      energy += (want - energy) * 0.18;

      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, size, size);
      const color = getComputedStyle(c).color;
      ctx.fillStyle = color;

      const step = size / 30;
      const r = size * (0.36 + 0.07 * energy);
      const mid = size / 2;
      for (let gy = step / 2; gy < size; gy += step) {
        for (let gx = step / 2; gx < size; gx += step) {
          const x = (gx - mid) / r;
          const y = (gy - mid) / r;
          const d2 = x * x + y * y;
          if (d2 >= 1) {
            // A faint field around the sphere.
            ctx.globalAlpha = 0.07;
            ctx.beginPath();
            ctx.arc(gx, gy, 0.6, 0, Math.PI * 2);
            ctx.fill();
            continue;
          }
          const z = Math.sqrt(1 - d2);
          // Light from the upper left, plus a ripple that travels outward.
          const lit = Math.max(0, -0.45 * x - 0.5 * y + 0.74 * z);
          const ripple = 0.5 + 0.5 * Math.sin(Math.sqrt(d2) * 9 - t * 4.5);
          const b = lit * (0.55 + 0.45 * energy) + ripple * energy * 0.35 * z;
          ctx.globalAlpha = Math.min(1, 0.12 + b);
          ctx.beginPath();
          ctx.arc(gx, gy, 0.5 + b * step * 0.2, 0, Math.PI * 2);
          ctx.fill();
        }
      }
      ctx.globalAlpha = 1;
      if (!still) raf = requestAnimationFrame(draw);
    };

    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return <canvas ref={canvas} className="orb" aria-hidden="true" />;
}
