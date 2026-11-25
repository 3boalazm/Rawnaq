import { useRef } from "react";

interface Props {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  /** Text shown under the knob, e.g. "+2.5 dB". */
  display: string;
  /** Where double-click returns to. */
  reset: number;
  /** Fill the arc from the middle (for ± values) instead of from the start. */
  bipolar?: boolean;
  size?: "md" | "lg";
  disabled?: boolean;
  onChange: (v: number) => void;
}

const START = -135;
const SWEEP = 270;

function polar(r: number, deg: number): [number, number] {
  const a = ((deg - 90) * Math.PI) / 180;
  return [50 + r * Math.cos(a), 50 + r * Math.sin(a)];
}

function arc(r: number, from: number, to: number) {
  if (Math.abs(to - from) < 0.01) return "";
  const [a, b] = from < to ? [from, to] : [to, from];
  const [x1, y1] = polar(r, a);
  const [x2, y2] = polar(r, b);
  return `M ${x1} ${y1} A ${r} ${r} 0 ${b - a > 180 ? 1 : 0} 1 ${x2} ${y2}`;
}

/** A rotary control: drag up/down, scroll, arrow keys, double-click to reset. */
export default function Knob(p: Props) {
  const drag = useRef<{ y: number; v: number } | null>(null);
  const frac = (p.value - p.min) / (p.max - p.min);
  const angle = START + frac * SWEEP;
  const origin = p.bipolar ? START + SWEEP / 2 : START;

  const set = (v: number) => {
    const snapped = Math.round(v / p.step) * p.step;
    const clamped = Math.min(p.max, Math.max(p.min, snapped));
    if (clamped !== p.value) p.onChange(Number(clamped.toFixed(4)));
  };

  const ticks = Array.from({ length: 19 }, (_, i) => START + (i * SWEEP) / 18);
  const [px, py] = polar(22, angle);

  return (
    <div className={`knob ${p.size ?? "md"} ${p.disabled ? "off" : ""}`}>
      <div
        className="knob-dial"
        role="slider"
        tabIndex={p.disabled ? -1 : 0}
        aria-label={p.label}
        aria-valuemin={p.min}
        aria-valuemax={p.max}
        aria-valuenow={p.value}
        aria-valuetext={p.display}
        aria-disabled={p.disabled}
        onPointerDown={(e) => {
          if (p.disabled) return;
          e.currentTarget.setPointerCapture(e.pointerId);
          drag.current = { y: e.clientY, v: p.value };
        }}
        onPointerMove={(e) => {
          if (!drag.current) return;
          const fine = e.shiftKey ? 0.25 : 1;
          set(drag.current.v + ((drag.current.y - e.clientY) / 160) * (p.max - p.min) * fine);
        }}
        onPointerUp={() => (drag.current = null)}
        onPointerCancel={() => (drag.current = null)}
        onDoubleClick={() => !p.disabled && p.onChange(p.reset)}
        onWheel={(e) => !p.disabled && set(p.value + (e.deltaY < 0 ? p.step : -p.step))}
        onKeyDown={(e) => {
          if (p.disabled) return;
          if (e.key === "ArrowUp" || e.key === "ArrowRight") set(p.value + p.step);
          if (e.key === "ArrowDown" || e.key === "ArrowLeft") set(p.value - p.step);
          if (e.key === "Home") p.onChange(p.reset);
        }}
      >
        <svg viewBox="0 0 100 100" aria-hidden="true">
          {ticks.map((t) => {
            const [x1, y1] = polar(46, t);
            const [x2, y2] = polar(49.5, t);
            const lit = p.bipolar
              ? (t >= Math.min(origin, angle) && t <= Math.max(origin, angle))
              : t <= angle;
            return <line key={t} x1={x1} y1={y1} x2={x2} y2={y2} className={lit ? "tick lit" : "tick"} />;
          })}
          <path d={arc(40, START, START + SWEEP)} className="track" />
          <path d={arc(40, origin, angle)} className="value" />
          <circle cx="50" cy="50" r="31" className="cap" />
          <circle cx="50" cy="50" r="24" className="cap-inner" />
          <circle cx={px} cy={py} r="2.2" className="pointer" />
        </svg>
      </div>
      <span className="knob-label">{p.label}</span>
      <span className="knob-value" dir="ltr">
        {p.display}
      </span>
    </div>
  );
}
