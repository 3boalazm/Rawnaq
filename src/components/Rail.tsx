export type Tool = "diagnose" | "repair" | "denoise" | "master" | "eq" | "space";

const TOOLS: { id: Tool; label: string; icon: JSX.Element }[] = [
  { id: "diagnose", label: "تشخيص", icon: <path d="M3 12h4l3-8 4 16 3-8h4" /> },
  {
    id: "repair",
    label: "إصلاح",
    icon: <path d="M14.5 5.5a4 4 0 0 0 4.8 4.8L10 19.6a1.9 1.9 0 0 1-2.7 0l-.9-.9a1.9 1.9 0 0 1 0-2.7l9.3-9.3a4 4 0 0 0-1.2-1.2z" />,
  },
  { id: "denoise", label: "تنقية", icon: <path d="M3 12c2-7 4-7 6 0s4 7 6 0 2-3.5 3-3.5 2 3.5 3 3.5" /> },
  {
    id: "master",
    label: "ماستر",
    icon: (
      <>
        <path d="M6 4v16M12 4v16M18 4v16" />
        <circle cx="6" cy="9" r="2.2" className="fill" />
        <circle cx="12" cy="15" r="2.2" className="fill" />
        <circle cx="18" cy="8" r="2.2" className="fill" />
      </>
    ),
  },
  {
    id: "eq",
    label: "معادل",
    icon: (
      <>
        <path d="M5 20V10M10 20V4M15 20v-8M20 20V7" />
      </>
    ),
  },
  {
    id: "space",
    label: "صدى",
    icon: (
      <>
        <path d="M4 20V11a8 8 0 0 1 16 0v9" />
        <path d="M9 20v-6a3 3 0 0 1 6 0v6M2.5 20h19" />
      </>
    ),
  },
];

interface Props {
  tool: Tool;
  disabled: boolean;
  onSelect: (t: Tool) => void;
}

/** The stages of the work, as a vertical tool rail. */
export default function Rail({ tool, disabled, onSelect }: Props) {
  return (
    <nav className="rail" aria-label="الأدوات">
      {TOOLS.map((t, i) => (
        <button
          key={t.id}
          className={tool === t.id && !disabled ? "on" : ""}
          aria-current={tool === t.id}
          disabled={disabled}
          onClick={() => onSelect(t.id)}
        >
          <span className="rail-icon">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              {t.icon}
            </svg>
          </span>
          <span className="rail-label">
            <em>{i + 1}</em>
            {t.label}
          </span>
        </button>
      ))}
    </nav>
  );
}
