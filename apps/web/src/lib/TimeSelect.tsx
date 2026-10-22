import { OptionSelect } from "@stockwatcher/chart-web/ui";

const pad = (n: number) => String(n).padStart(2, "0");
const HOURS = Array.from({ length: 24 }, (_, h) => ({ value: pad(h), label: pad(h) }));
const STEP_MINUTES = Array.from({ length: 12 }, (_, i) => pad(i * 5));

/**
 * A 24-hour "HH:MM" picked with two shadcn Selects, hour and minute, in place
 * of `<input type="time">` (owner, 2026-10-22: no browser-native UI; the
 * native time field also renders differently in every engine and in the
 * Tauri webview). Minutes step by 5, and a stored value off that grid (set
 * before this control, or by sync) stays selectable rather than being
 * silently rounded. The value in and out is the same "HH:MM" string the
 * native input used.
 */
export function TimeSelect({ label, value, onChange }: { label: string; value: string; onChange: (hhmm: string) => void }) {
  const [h = "00", m = "00"] = value.split(":");
  const minutes = (STEP_MINUTES.includes(m) ? STEP_MINUTES : [...STEP_MINUTES, m].sort()).map((v) => ({ value: v, label: v }));
  return (
    <span role="group" aria-label={label} className="inline-flex items-center gap-0.5 font-mono">
      <OptionSelect size="sm" aria-label={`${label}, hour`} options={HOURS} value={h} onValueChange={(v) => onChange(`${v}:${m}`)} className="bg-background" contentClassName="font-mono" />
      <span aria-hidden="true">:</span>
      <OptionSelect size="sm" aria-label={`${label}, minute`} options={minutes} value={m} onValueChange={(v) => onChange(`${h}:${v}`)} className="bg-background" contentClassName="font-mono" />
    </span>
  );
}
