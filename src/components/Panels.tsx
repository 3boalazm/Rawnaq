import type { Session } from "../hooks/useSession";
import { ISSUE_LABEL, LOUDNESS_LEVELS, NOISE_KIND, signed } from "../lib/format";
import Knob from "./Knob";
import Switch from "./Switch";
import ToneCurve from "./ToneCurve";

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="stat">
      <span>{label}</span>
      <b dir="ltr">{value}</b>
    </div>
  );
}

export function DiagnosePanel({ s }: { s: Session }) {
  const d = s.loaded!.diagnosis;
  return (
    <>
      <div className="stats">
        <Stat label="أرضية الضوضاء" value={`${d.noise_floor_dbfs.toFixed(0)} dBFS`} />
        <Stat label="الإشارة / الضوضاء" value={`${d.snr_db.toFixed(0)} dB`} />
        <Stat label="القمة" value={`${d.peak_dbfs.toFixed(1)} dBFS`} />
        <Stat label="الجهارة" value={`${d.loudness_lufs.toFixed(1)} LUFS`} />
      </div>
      <h3>ما رُصد</h3>
      {d.issues.length === 0 ? (
        <p className="fine">لم يُرصد تلف واضح.</p>
      ) : (
        <ul className="issues">
          {d.issues.map((i) => (
            <li key={i}>
              {ISSUE_LABEL[i] ?? i}
              {i === "hum" && d.mains_hum_hz ? ` · ${d.mains_hum_hz} Hz` : ""}
              {i === "dropouts" ? ` · ${d.dropouts}` : ""}
              {i === "clipping" ? ` · ${d.clipped_runs}` : ""}
            </li>
          ))}
        </ul>
      )}
      <p className="fine">الخطوات التالية مضبوطة مسبقًا على ما رُصد هنا. يمكنك الترميم مباشرة.</p>
    </>
  );
}

export function RepairPanel({ s }: { s: Session }) {
  const r = s.settings.repair;
  const off = !!s.busy;
  return (
    <>
      <Switch
        label="ترميم الانقطاعات"
        help="يعيد بناء الفجوات القصيرة من جوارها"
        checked={r.fill_dropouts}
        disabled={off}
        onChange={(v) => s.patchRepair({ fill_dropouts: v })}
      />
      <Switch
        label="إزالة الطقطقة"
        help="نقرات وفرقعات الأشرطة والأسطوانات"
        checked={r.declick}
        disabled={off}
        onChange={(v) => s.patchRepair({ declick: v })}
      />
      <Switch
        label="إزالة الطنين الكهربائي"
        help="لا يغيّر شيئًا إن لم يوجد طنين"
        checked={r.dehum}
        disabled={off}
        onChange={(v) => s.patchRepair({ dehum: v })}
      />
      <Switch
        label="إصلاح القمم المقصوصة"
        help="بطيء في التسجيلات الطويلة"
        checked={r.declip}
        disabled={off}
        onChange={(v) => s.patchRepair({ declip: v })}
      />
      <p className="fine">إصلاحات تقليدية: تعيد بناء المواضع التالفة من جوارها ولا تولّد صوتًا.</p>
    </>
  );
}

export function DenoisePanel({ s }: { s: Session }) {
  const { settings, busy, processed } = s;
  const q = processed?.quality;
  const noise = processed?.noise;
  const off = !!busy || !settings.denoise;
  return (
    <>
      <Switch
        label="تنقية الضوضاء"
        help="كاشف صوت · كاشف تشويش · DeepFilterNet 3"
        checked={settings.denoise}
        disabled={!!busy}
        onChange={(v) => s.patch({ denoise: v })}
      />
      <div className="knob-row three">
        <Knob
          label="أرضية التشويش"
          value={Math.round(settings.hiss * 100)}
          min={0}
          max={100}
          step={1}
          reset={60}
          display={`${Math.round(settings.hiss * 100)}%`}
          disabled={off}
          onChange={(v) => s.patch({ hiss: v / 100 })}
        />
        <Knob
          label="الشدة"
          value={Math.round(settings.strength * 100)}
          min={0}
          max={100}
          step={1}
          reset={85}
          display={`${Math.round(settings.strength * 100)}%`}
          disabled={off}
          onChange={(v) => s.patch({ strength: v / 100 })}
        />
        <Knob
          label="حماية الصوت"
          value={Math.round(settings.protect * 100)}
          min={0}
          max={100}
          step={1}
          reset={90}
          display={`${Math.round(settings.protect * 100)}%`}
          disabled={off}
          onChange={(v) => s.patch({ protect: v / 100 })}
        />
      </div>
      <p className="fine">
        «أرضية التشويش» تخفض الضجيج الثابت المقيس في كل التسجيل، حتى تحت الصوت. «الشدة» تنقية أعمق
        تعمل كاملة في الفواصل، و«حماية الصوت» تكفّها حيث يُرصد صوت بشري.
      </p>
      {processed && noise && (
        <>
          <h3>الكاشفان</h3>
          <div className="detectors">
            <div className="detector voice">
              <span>الصوت البشري</span>
              <b>{(processed.voice_share * 100).toFixed(0)}%</b>
              <small>من زمن التسجيل</small>
            </div>
            <div className="detector noise">
              <span>التشويش الثابت</span>
              <b>{noise.level_dbfs.toFixed(0)} dB</b>
              <small>{NOISE_KIND[noise.kind]}</small>
            </div>
          </div>
          <div className="bands" aria-label="توزيع التشويش على النطاقات">
            {(
              [
                ["منخفض", noise.low_dbfs],
                ["متوسط", noise.mid_dbfs],
                ["عالٍ", noise.high_dbfs],
              ] as const
            ).map(([label, db]) => (
              <div className="band" key={label}>
                <span>{label}</span>
                <i>
                  <em style={{ width: `${Math.max(4, Math.min(100, ((db + 90) / 70) * 100))}%` }} />
                </i>
                <b>{db.toFixed(0)}</b>
              </div>
            ))}
          </div>
        </>
      )}
      {q && (
        <>
          <h3>القياس</h3>
          <div className="stats">
            <Stat label="تخفيف الضوضاء" value={`${q.noise_reduction_db.toFixed(1)} dB`} />
            <Stat label="فقد في الصوت" value={`${q.voice_loss_db.toFixed(2)} dB`} />
            <Stat label="مقاطع مشكوك فيها" value={`${(q.flagged_ratio * 100).toFixed(1)}%`} />
            <Stat label="قمة المحذوف" value={`${processed!.removed_peak_dbfs.toFixed(0)} dB`} />
          </div>
        </>
      )}
    </>
  );
}

export function MasterPanel({ s }: { s: Session }) {
  const { settings, busy, processed, loaded } = s;
  const e = settings.enhance;
  const off = !!busy;
  const mono = loaded!.diagnosis.channels === 1;
  // What the automatic compensation will add, from the last measurement.
  const autoLow = e.tone_restore && processed ? Math.min(6, Math.max(0, processed.enhance.low_loss_db)) : 0;
  const autoHigh = e.tone_restore && processed ? Math.min(4, Math.max(0, processed.enhance.high_loss_db)) : 0;

  return (
    <>
      <div className="master">
        <Knob
          bipolar
          label="الدفء"
          value={e.warmth_db}
          min={-6}
          max={6}
          step={0.5}
          reset={0}
          display={signed(e.warmth_db)}
          disabled={off}
          onChange={(v) => s.patchEnhance({ warmth_db: v })}
        />
        <Knob
          size="lg"
          bipolar
          label="الصوت الرئيسي"
          value={e.master_db}
          min={-12}
          max={12}
          step={0.5}
          reset={0}
          display={signed(e.master_db)}
          disabled={off}
          onChange={(v) => s.patchEnhance({ master_db: v })}
        />
        <Knob
          bipolar
          label="الوضوح"
          value={e.clarity_db}
          min={-6}
          max={6}
          step={0.5}
          reset={0}
          display={signed(e.clarity_db)}
          disabled={off}
          onChange={(v) => s.patchEnhance({ clarity_db: v })}
        />
      </div>

      <ToneCurve low={autoLow + e.warmth_db} high={autoHigh + e.clarity_db} />
      {processed && settings.denoise && (
        <p className="fine">
          أخذت التنقية: جهير <b dir="ltr">{signed(-processed.enhance.low_loss_db)}</b> · وضوح{" "}
          <b dir="ltr">{signed(-processed.enhance.high_loss_db)}</b>
        </p>
      )}

      <Switch
        label="تعويض النغمة تلقائيًا"
        help="يعيد ما أخذته التنقية من الجهير والوضوح"
        checked={e.tone_restore}
        disabled={off}
        onChange={(v) => s.patchEnhance({ tone_restore: v })}
      />
      <Switch
        label="رفع مستوى الصوت"
        help="بعد إزالة الضوضاء، فلا ترتفع معه"
        checked={e.loudness}
        disabled={off}
        onChange={(v) => s.patchEnhance({ loudness: v })}
      />
      <div className="chips" role="radiogroup" aria-label="مستوى الصوت">
        {LOUDNESS_LEVELS.map(([lufs, label]) => (
          <button
            key={lufs}
            role="radio"
            aria-checked={e.target_lufs === lufs}
            className={e.target_lufs === lufs ? "on" : ""}
            disabled={off || !e.loudness}
            onClick={() => s.patchEnhance({ target_lufs: lufs })}
          >
            {label}
          </button>
        ))}
      </div>
      <Switch
        label="قناتان (يمين وشمال)"
        help={mono ? "ينسخ الصوت الأحادي إلى القناتين" : "التسجيل بقناتين أصلًا"}
        checked={e.stereo}
        disabled={off || !mono}
        onChange={(v) => s.patchEnhance({ stereo: v })}
      />
      {processed && (
        <p className="fine">
          المطبَّق: رفع <b dir="ltr">{signed(processed.enhance.gain_db)}</b>
          {processed.enhance.limited_db > 0.1 && (
            <>
              {" "}
              · كبح القمم <b dir="ltr">{processed.enhance.limited_db.toFixed(1)} dB</b>
            </>
          )}
        </p>
      )}
    </>
  );
}
