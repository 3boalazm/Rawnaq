import { useEffect, useState } from "react";
import Orb from "./components/Orb";
import { DenoisePanel, DiagnosePanel, EqPanel, MasterPanel, RepairPanel, SpacePanel } from "./components/Panels";
import Rail, { type Tool } from "./components/Rail";
import { SHORTCUTS, useShortcuts } from "./hooks/useShortcuts";
import Timeline, { type Track } from "./components/Timeline";
import { useSession } from "./hooks/useSession";
import { ENGINES, formatTime, overallProgress, remaining, STAGE_LABEL, VERDICT } from "./lib/format";
import type { Peaks } from "./types";

const TOOL_TITLE: Record<Tool, [string, string]> = {
  diagnose: ["التشخيص", "حالة التسجيل قبل أي معالجة"],
  repair: ["الإصلاح", "علاج التلف الميكانيكي والكهربائي"],
  denoise: ["التنقية", "إزالة الضوضاء مع حماية صوت القارئ"],
  master: ["الماستر", "النغمة والمستوى النهائي"],
  eq: ["المعادل", "تشكيل نغمة الصوت بالترددات"],
  space: ["الصدى", "المكان الذي تُسمع فيه التلاوة"],
};

const MODE_LABEL = { original: "الأصل", restored: "بعد الترميم", removed: "المحذوف" } as const;

/** Signal level at `time`, from the waveform envelope. */
function levelAt(peaks: Peaks | null | undefined, time: number, duration: number) {
  if (!peaks || duration <= 0) return 0;
  const i = Math.min(peaks.max.length - 1, Math.max(0, Math.floor((time / duration) * peaks.max.length)));
  return Math.max(peaks.max[i], -peaks.min[i]);
}

export default function App() {
  const s = useSession();
  const [tool, setTool] = useState<Tool>("diagnose");
  const [help, setHelp] = useState(false);
  useShortcuts(s, (open) => setHelp((h) => open ?? !h));
  const { loaded, processed, busy, mode } = s;
  const d = loaded?.diagnosis;
  const q = processed?.quality ?? null;

  const tracks: Track[] = [
    { id: "original", label: "الأصل", hint: "التسجيل كما هو", peaks: loaded?.peaks ?? null, color: "--track-original" },
    { id: "restored", label: "المرمَّم", hint: "الناتج بعد الترميم", peaks: processed?.peaks ?? null, color: "--track-restored" },
    { id: "removed", label: "المحذوف", hint: "ما أزالته التنقية", peaks: processed?.removed_peaks ?? null, color: "--track-removed" },
  ];
  const activePeaks = tracks.find((t) => t.id === mode)?.peaks;

  const tone = q ? q.verdict : "idle";
  // One bar for the whole restoration, and how long is left.
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!busy) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [!!busy]);
  const overall = busy ? overallProgress(busy.stage, busy.fraction, s.settings.engine) : null;
  const left = busy ? remaining(overall, now - busy.startedAt) : null;
  const restoring = !!busy && busy.stage !== "load";

  const runLabel = busy
    ? `${STAGE_LABEL[busy.stage] ?? busy.stage}…`
    : !s.applied
      ? "رمِّم التسجيل"
      : s.dirty
        ? "طبّق التغييرات"
        : "أعد الترميم";

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="logo" aria-hidden="true">
            {[10, 18, 26, 14, 22, 8].map((h, i) => (
              <i key={i} style={{ height: h }} />
            ))}
          </span>
          <h1>رونق</h1>
        </div>

        <div className="crumb">
          {loaded ? (
            <>
              <span>التسجيلات</span>
              <i>/</i>
              <strong dir="auto">{loaded.file_name}</strong>
            </>
          ) : (
            <span>ترميم التلاوات القديمة · كل المعالجة على جهازك</span>
          )}
        </div>

        <div className="topbar-actions">
          <button className="btn" onClick={s.pickFile} disabled={!!busy}>
            {loaded ? "تسجيل آخر" : "فتح تسجيل"}
          </button>
          <button className="btn primary" onClick={s.exportFile} disabled={!processed || !!busy || s.dirty || s.syncing}>
            تصدير
          </button>
        </div>
      </header>

      {s.error && (
        <div className="banner error" role="alert">
          {s.error}
        </div>
      )}
      {s.notice && <div className="banner">{s.notice}</div>}

      <div className={`workspace ${loaded ? "" : "bare"}`}>
        <Rail tool={tool} disabled={!loaded} onSelect={setTool} />

        <main className="center">
          <section className={`hero ${tone}`}>
            <div className="hero-glow" aria-hidden="true" />
            <Orb level={levelAt(activePeaks, s.time, d?.duration_sec ?? 0)} active={s.playing} />

            {!loaded ? (
              <button className="hero-drop" onClick={s.pickFile} disabled={!!busy}>
                <strong>{busy ? STAGE_LABEL.load : "اسحب التسجيل هنا"}</strong>
                <span>أو اضغط للاختيار · WAV · MP3 · FLAC · OGG · M4A</span>
              </button>
            ) : (
              <>
                <div className="hero-read">
                  <span className="eyebrow">{q ? "تخفيف الضوضاء" : "نسبة الإشارة إلى الضوضاء"}</span>
                  <div className="readout" dir="ltr">
                    <span className="dots">{(q ? q.noise_reduction_db : d!.snr_db).toFixed(1)}</span>
                    <small>dB</small>
                  </div>
                  <h2>{q ? VERDICT[q.verdict].title : "جاهز للترميم"}</h2>
                  <p>
                    {q
                      ? VERDICT[q.verdict].body
                      : "راجع الأدوات الأربع في الشريط الجانبي، أو اضغط «رمِّم التسجيل» بالإعدادات المقترحة."}
                  </p>
                </div>

                <div className="hero-meta" dir="ltr">
                  <span>{formatTime(d!.duration_sec)}</span>
                  <span>{(d!.sample_rate / 1000).toFixed(1)} kHz</span>
                  <span>{d!.channels === 1 ? "Mono" : d!.channels === 2 ? "Stereo" : `${d!.channels} ch`}</span>
                </div>

                <div className="transport">
                  <button className="play" onClick={s.togglePlay} aria-label={s.playing ? "إيقاف" : "تشغيل"}>
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                      {s.playing ? (
                        <>
                          <rect x="6" y="5" width="4" height="14" rx="1" />
                          <rect x="14" y="5" width="4" height="14" rx="1" />
                        </>
                      ) : (
                        <path d="M8 5.5v13a1 1 0 0 0 1.5.86l11-6.5a1 1 0 0 0 0-1.72l-11-6.5A1 1 0 0 0 8 5.5z" />
                      )}
                    </svg>
                  </button>
                  <span className="clock" dir="ltr">
                    {formatTime(s.time)} <i>/ {formatTime(d!.duration_sec)}</i>
                  </span>
                  <span className="listening" dir="rtl">
                    تسمع الآن: <b>{MODE_LABEL[mode]}</b>
                  </span>
                  <button
                    className={s.muted || s.volume === 0 ? "mini on" : "mini"}
                    onClick={s.toggleMute}
                    aria-pressed={s.muted}
                    aria-label={s.muted ? "إلغاء الكتم" : "كتم الصوت"}
                    title={`مستوى الاستماع ${Math.round(s.volume * 100)}% · M للكتم · ↑ ↓ للمستوى`}
                  >
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                      <path d="M4 9.5h3.5L12 6v12l-4.5-3.5H4z" />
                      {s.muted || s.volume === 0 ? (
                        <path d="M16 9.5l5 5M21 9.5l-5 5" />
                      ) : (
                        <path d={s.volume > 0.5 ? "M15.5 8.5a5 5 0 0 1 0 7M18 6a8.5 8.5 0 0 1 0 12" : "M15.5 8.5a5 5 0 0 1 0 7"} />
                      )}
                    </svg>
                  </button>
                  <button className="mini" onClick={() => setHelp((h) => !h)} aria-expanded={help} aria-label="اختصارات لوحة المفاتيح" title="اختصارات لوحة المفاتيح (?)">
                    <b>?</b>
                  </button>
                </div>
              </>
            )}

            {busy && (
              <div className="progress" role="status" aria-label={STAGE_LABEL[busy.stage] ?? busy.stage}>
                <div style={{ width: `${Math.round((overall ?? busy.fraction) * 100)}%` }} />
              </div>
            )}
            {restoring && (
              <div className="restoring" role="status">
                <b dir="ltr">{Math.round((overall ?? 0) * 100)}%</b>
                <span>{STAGE_LABEL[busy!.stage] ?? busy!.stage}</span>
                <small>{left ? `متبقي ${left}` : "جارٍ تقدير الوقت…"}</small>
              </div>
            )}
            {!busy && s.syncing && (
              <div className="progress" role="status" aria-label="تطبيق التغييرات">
                <div style={{ width: `${Math.max(4, Math.round(s.syncInfo.fraction * 100))}%` }} />
              </div>
            )}
          </section>

          {loaded && (
            <Timeline
              tracks={tracks}
              active={mode}
              duration={d!.duration_sec}
              time={s.time}
              voice={processed?.voice_regions ?? []}
              flagged={q?.flagged_regions ?? []}
              onSelect={s.setMode}
              onSeek={s.seek}
            />
          )}
        </main>

        {loaded && (
          <aside className="inspector">
            <header>
              <h2>{TOOL_TITLE[tool][0]}</h2>
              <p>{TOOL_TITLE[tool][1]}</p>
            </header>
            <div className="inspector-body">
              {tool === "diagnose" && <DiagnosePanel s={s} />}
              {tool === "repair" && <RepairPanel s={s} />}
              {tool === "denoise" && <DenoisePanel s={s} />}
              {tool === "master" && <MasterPanel s={s} />}
              {tool === "eq" && <EqPanel s={s} />}
              {tool === "space" && <SpacePanel s={s} />}
            </div>
            <footer>
              <div className="engines" role="radiogroup" aria-label="طريقة الترميم">
                {ENGINES.map((e) => {
                  const missing = e.id === "mossformer" && !s.deepAvailable;
                  return (
                    <button
                      key={e.id}
                      role="radio"
                      aria-checked={s.settings.engine === e.id}
                      className={s.settings.engine === e.id ? "on" : ""}
                      disabled={!!busy || missing || !s.settings.denoise}
                      onClick={() => s.patch({ engine: e.id })}
                    >
                      <b>{e.label}</b>
                      <small>{missing ? "غير مثبّت" : e.help}</small>
                    </button>
                  );
                })}
              </div>
              {restoring ? (
                <button className="btn big" onClick={s.cancel}>
                  إلغاء الترميم
                </button>
              ) : (
                <button className="btn primary big" onClick={s.run} disabled={!!busy}>
                  {runLabel}
                </button>
              )}
              {s.syncing && (
                <div className="applying" role="status">
                  <div className="applying-head">
                    <span>
                      {s.syncInfo.changes.length > 1
                        ? `يُطبَّق ${s.syncInfo.changes.length} تغييرات`
                        : "يُطبَّق التغيير"}
                    </span>
                    <b dir="ltr">{Math.round(s.syncInfo.fraction * 100)}%</b>
                  </div>
                  <i aria-hidden="true">
                    <em style={{ width: `${Math.max(4, Math.round(s.syncInfo.fraction * 100))}%` }} />
                  </i>
                  <ol>
                    {s.syncInfo.changes.map((c, i) => (
                      <li key={c}>
                        <small dir="ltr">
                          {i + 1}/{s.syncInfo.changes.length}
                        </small>
                        {c}
                      </li>
                    ))}
                  </ol>
                </div>
              )}
              <span className="fine">
                {!s.applied
                  ? "الإعدادات مقترحة حسب التشخيص."
                  : s.syncing
                    ? "تُطبَّق معًا في مرّة واحدة."
                    : s.dirty
                      ? "هذا التغيير يحتاج إعادة الترميم."
                      : "التنقية والماستر يُطبَّقان تلقائيًا."}
              </span>
            </footer>
          </aside>
        )}
      </div>

      {help && (
        <div className="sheet-backdrop" onClick={() => setHelp(false)}>
          <div className="sheet" role="dialog" aria-label="اختصارات لوحة المفاتيح" onClick={(e) => e.stopPropagation()}>
            <h3>اختصارات لوحة المفاتيح</h3>
            <dl>
              {SHORTCUTS.map(([keys, what]) => (
                <div key={keys}>
                  <dt dir="ltr">{keys}</dt>
                  <dd>{what}</dd>
                </div>
              ))}
            </dl>
            <button className="btn" onClick={() => setHelp(false)}>
              تم
            </button>
          </div>
        </div>
      )}

      <audio {...s.audioProps} />
    </div>
  );
}
