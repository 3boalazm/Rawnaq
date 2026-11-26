import type { BodyStyle, Engine, ReverbStyle, Settings, Verdict } from "../types";

export const AUDIO_EXTENSIONS = [
  "wav", "mp3", "flac", "ogg", "m4a", "aac", "aiff", "aif", "opus", "mp4", "mkv",
];

export const DEFAULT_SETTINGS: Settings = {
  repair: { fill_dropouts: true, declick: true, declick_threshold: 10, dehum: true, declip: false },
  denoise: true,
  engine: "deepfilter",
  hiss: 0.6,
  strength: 0.85,
  protect: 0.9,
  enhance: {
    tone_restore: true,
    warmth_db: 0,
    clarity_db: 0,
    loudness: true,
    target_lufs: -18,
    eq_db: [0, 0, 0, 0, 0],
    master_db: 0,
    stereo: false,
    body: "none",
    body_amount: 0.4,
    reverb: "none",
    reverb_amount: 0.4,
  },
};

export const STAGE_LABEL: Record<string, string> = {
  load: "قراءة التسجيل وتشخيصه",
  repair: "إصلاح التلف",
  detect: "رصد الصوت البشري وقياس التشويش",
  denoise: "تنقية الضوضاء",
  finish: "فحص الجودة وتجهيز المعاينة",
};

export const ISSUE_LABEL: Record<string, string> = {
  noise: "ضوضاء خلفية مرتفعة",
  clipping: "قصّ في القمم (تشبّع)",
  hum: "طنين كهربائي",
  dropouts: "انقطاعات رقمية قصيرة",
};

export const VERDICT: Record<Verdict, { title: string; body: string }> = {
  ok: { title: "صوت القارئ لم يتأثر", body: "لم يُرصد فقد يُذكر في مقاطع التلاوة." },
  caution: {
    title: "استمع قبل الاعتماد",
    body: "رُصد فقد بسيط في بعض المقاطع. استمع إلى مسار «المحذوف» وتأكد أنه خالٍ من صوت القارئ.",
  },
  warn: {
    title: "التنقية أخذت من صوت القارئ",
    body: "ارفع «حماية الصوت» أو خفّف الشدة، ثم استمع إلى «المحذوف» عند العلامات الحمراء.",
  },
  unknown: {
    title: "تعذّر الحكم آليًا",
    body: "لم تُميَّز مقاطع تلاوة كافية للقياس. اعتمد على الاستماع إلى مسار «المحذوف».",
  },
};

export const ENGINES: { id: Engine; label: string; help: string }[] = [
  { id: "deepfilter", label: "سريع", help: "ينظّف الفواصل ويترك ما تحت الصوت" },
  { id: "mossformer", label: "عميق", help: "ينظّف تحت الصوت أيضًا، أبطأ" },
];

export const BODIES: { id: BodyStyle; label: string; help: string }[] = [
  { id: "none", label: "بدون", help: "الصوت كما هو" },
  { id: "thicken", label: "تضخيم", help: "للصوت الرفيع: وزن وجهير ودفء" },
  { id: "tame", label: "ترويض", help: "للصوت الضخم: يهدّئ الدمدمة والضربات" },
];

export const TONE_VERDICT: Record<string, string> = {
  thin: "يميل إلى الرفيع",
  balanced: "متوازن",
  boomy: "يميل إلى الضخم",
};

export const REVERBS: { id: ReverbStyle; label: string; help: string }[] = [
  { id: "none", label: "بدون", help: "الصوت كما هو" },
  { id: "room", label: "مصلّى", help: "قريب ودافئ، صدى قصير" },
  { id: "mosque", label: "مسجد", help: "صدى التلاوة المألوف" },
  { id: "grand", label: "جامع كبير", help: "قبّة عالية، صدى طويل وعميق" },
  { id: "hall", label: "قاعة", help: "طويل لكنه واضح ولامع" },
  { id: "vast", label: "فضاء", help: "واسع وبعيد، ينقل التلاوة لمكان آخر" },
];

export const NOISE_KIND: Record<string, string> = {
  low: "دمدمة وطنين منخفض",
  mid: "ضجيج محيط (قاعة أو جمهور)",
  high: "هسيس الشريط",
};

export const LOUDNESS_LEVELS: [number, string][] = [
  [-23, "هادئ"],
  [-18, "متوسط"],
  [-14, "عالٍ"],
  [-10, "أقصى"],
];

export const EQ_BANDS = ["100", "300", "1k", "3k", "8k"];

export const EQ_PRESETS: { label: string; db: number[] }[] = [
  { label: "مسطّح", db: [0, 0, 0, 0, 0] },
  { label: "دفء", db: [4, 2, 0, -1, -2] },
  { label: "حضور", db: [-2, -1, 2, 4, 1] },
  { label: "وضوح", db: [-3, -2, 0, 3, 5] },
  { label: "جهير", db: [6, 3, 0, 0, 0] },
  { label: "بريق", db: [0, 0, 0, 2, 6] },
  { label: "هادئ", db: [1, 0, 0, -3, -5] },
  { label: "راديو قديم", db: [-8, -2, 4, 3, -8] },
];

export function formatTime(t: number) {
  const s = Math.max(0, Math.floor(t));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

export const signed = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(1)} dB`;
