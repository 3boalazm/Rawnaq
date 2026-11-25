interface Props {
  /** Low-shelf gain at 250 Hz, dB. */
  low: number;
  /** High-shelf gain at 3 kHz, dB. */
  high: number;
}

const W = 300;
const H = 120;
const RANGE = 12; // ±dB shown
const F_MIN = 30;
const F_MAX = 16000;

const x = (f: number) => (Math.log(f / F_MIN) / Math.log(F_MAX / F_MIN)) * W;
const y = (db: number) => H / 2 - (db / RANGE) * (H / 2 - 8);

/** The tone shaping applied to the voice: a low shelf and a high shelf. */
export default function ToneCurve({ low, high }: Props) {
  const gain = (f: number) => low / (1 + (f / 250) ** 2) + high / (1 + (3000 / f) ** 2);
  const pts = Array.from({ length: 80 }, (_, i) => {
    const f = F_MIN * (F_MAX / F_MIN) ** (i / 79);
    return `${x(f).toFixed(1)},${y(gain(f)).toFixed(1)}`;
  });
  const line = `M ${pts.join(" L ")}`;

  return (
    <svg className="tone-curve" viewBox={`0 0 ${W} ${H}`} direction="ltr" role="img" aria-label="منحنى النغمة">
      <defs>
        <linearGradient id="tone-fill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="var(--accent)" stopOpacity="0.35" />
          <stop offset="1" stopColor="var(--accent)" stopOpacity="0" />
        </linearGradient>
      </defs>
      {[-RANGE / 2, 0, RANGE / 2].map((g) => (
        <line key={g} x1="0" x2={W} y1={y(g)} y2={y(g)} className={g === 0 ? "grid zero" : "grid"} />
      ))}
      {[100, 1000, 10000].map((f) => (
        <line key={f} x1={x(f)} x2={x(f)} y1="0" y2={H} className="grid" />
      ))}
      <path d={`${line} L ${W},${y(0)} L 0,${y(0)} Z`} fill="url(#tone-fill)" />
      <path d={line} className="curve" />
      <circle cx={x(120)} cy={y(gain(120))} r="4" className="node" />
      <circle cx={x(6000)} cy={y(gain(6000))} r="4" className="node" />
      <text x="4" y="12" className="axis">+{RANGE} dB</text>
      <text x="4" y={H - 4} className="axis">−{RANGE} dB</text>
      <text x={x(100) + 3} y={H - 4} className="axis">100</text>
      <text x={x(1000) + 3} y={H - 4} className="axis">1k</text>
      <text x={x(10000) + 3} y={H - 4} className="axis">10k</text>
    </svg>
  );
}
