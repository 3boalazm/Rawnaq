interface Props {
  label: string;
  help?: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}

export default function Switch({ label, help, checked, disabled, onChange }: Props) {
  return (
    <label className="switch">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <i aria-hidden="true" />
      <span>
        {label}
        {help && <small>{help}</small>}
      </span>
    </label>
  );
}
