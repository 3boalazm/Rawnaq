import { useEffect, useRef } from "react";
import type { Session } from "./useSession";

/** The shortcuts, as shown in the help sheet. */
export const SHORTCUTS: [string, string][] = [
  ["Space", "تشغيل / إيقاف"],
  ["←  →", "رجوع / تقديم 5 ثوانٍ"],
  ["Shift + ←  →", "رجوع / تقديم 30 ثانية"],
  ["Home  /  End", "أول التسجيل / آخره"],
  ["↑  ↓", "رفع / خفض مستوى الاستماع"],
  ["M", "كتم الصوت"],
  ["1  2  3", "الأصل / المرمَّم / المحذوف"],
  ["A", "بدّل بين الأصل والمرمَّم"],
  ["Ctrl + Enter", "رمّم أو طبّق التغييرات"],
  ["Ctrl + O", "فتح تسجيل"],
  ["Ctrl + S", "تصدير"],
  ["?", "إظهار هذه القائمة (Esc للإغلاق)"],
];

/**
 * Keyboard control of playback and the main actions.
 *
 * Keys are left alone while typing in a field. Arrows and Home/End are also
 * left to a focused knob, fader or timeline, which use them for their own
 * value. Space always plays and pauses, even with a button focused: the
 * button's own "click on Space" is cancelled so it does not fire as well.
 */
export function useShortcuts(s: Session, toggleHelp: (open?: boolean) => void) {
  // The handler reads the latest session without re-subscribing on every render.
  const live = useRef({ s, toggleHelp });
  live.current = { s, toggleHelp };

  useEffect(() => {
    const typing = (t: EventTarget | null) =>
      t instanceof HTMLElement &&
      (t.isContentEditable ||
        t.tagName === "TEXTAREA" ||
        t.tagName === "SELECT" ||
        (t.tagName === "INPUT" && (t as HTMLInputElement).type !== "range"));
    const adjusting = (t: EventTarget | null) =>
      t instanceof HTMLElement &&
      (t.getAttribute("role") === "slider" || (t.tagName === "INPUT" && (t as HTMLInputElement).type === "range"));

    const down = (e: KeyboardEvent) => {
      const { s, toggleHelp } = live.current;
      if (typing(e.target) || e.altKey || e.metaKey) return;
      const ready = !!s.loaded && !s.busy;
      const duration = s.loaded?.diagnosis.duration_sec ?? 0;
      const done = () => e.preventDefault();

      if (e.ctrlKey) {
        const key = e.key.toLowerCase();
        if (key === "o" && !s.busy) return done(), void s.pickFile();
        if (key === "s" && s.processed && !s.busy && !s.dirty && !s.syncing) return done(), void s.exportFile();
        if (e.key === "Enter" && ready) return done(), void s.run();
        return;
      }
      if (e.key === "?") return done(), toggleHelp();
      if (e.key === "Escape") return toggleHelp(false);
      if (!s.loaded) return;

      if (e.code === "Space" || e.key.toLowerCase() === "k") return done(), s.togglePlay();
      if (e.key.toLowerCase() === "m") return done(), s.toggleMute();
      if (e.key === "1") return done(), s.setMode("original");
      if (e.key === "2" && s.processed) return done(), s.setMode("restored");
      if (e.key === "3" && s.processed) return done(), s.setMode("removed");
      if (e.key.toLowerCase() === "a" && s.processed)
        return done(), s.setMode(s.mode === "original" ? "restored" : "original");

      if (adjusting(e.target)) return;
      const step = e.shiftKey ? 30 : 5;
      const to = (t: number) => s.seek(Math.max(0, Math.min(duration, t)));
      if (e.key === "ArrowRight") return done(), to(s.time + step);
      if (e.key === "ArrowLeft") return done(), to(s.time - step);
      if (e.key === "Home") return done(), to(0);
      if (e.key === "End") return done(), to(duration);
      if (e.key === "ArrowUp") return done(), s.setVolume(s.volume + 0.1);
      if (e.key === "ArrowDown") return done(), s.setVolume(s.volume - 0.1);
    };
    // A focused button clicks itself when Space is released; Space is ours.
    const up = (e: KeyboardEvent) => {
      if (e.code === "Space" && !typing(e.target) && live.current.s.loaded) e.preventDefault();
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, []);
}
