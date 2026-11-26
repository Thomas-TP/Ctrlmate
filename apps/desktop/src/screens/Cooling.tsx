import { useEffect, useMemo, useRef, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { CurveChart, type CurveThreshold } from '../components/charts/CurveChart';
import { Button } from '../components/ui/Button';
import { Segmented } from '../components/ui/Form';
import { Badge, Banner, Card, EmptyState, KeyValue, PageHeader } from '../components/ui/Layout';
import { cmd } from '../api/daemon';
import { useI18n } from '../i18n';
import { presetCurve, readSensor, type SensorReading } from '../lib/snapshot';
import { useActions } from '../store/actions';
import { useNav } from '../store/nav';
import {
  CPU_CRITICAL_MAX_C,
  CPU_CRITICAL_MIN_C,
  COOLING_PRESETS,
  ENTITY,
  MIN_PUMP_PERCENT,
  type CoolingPreset,
  type DaemonSnapshot,
} from '../types/protocol';
import { cn } from '../components/ui/Button';

type Source = 'cpu' | 'gpu' | 'liquid';
const SOURCE_ENTITY: Record<Source, string> = {
  cpu: ENTITY.cpuTemp,
  gpu: ENTITY.gpuTemp,
  liquid: ENTITY.liquidTemp,
};

export function Cooling({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t, code, temp, pct, unit, locale } = useI18n();
  const actions = useActions();
  const setScreen = useNav((s) => s.setScreen);
  const policy = snap.cooling_policy;
  const [pending, setPending] = useState<CoolingPreset | null>(null);
  const [preview, setPreview] = useState<CoolingPreset | null>(null);
  const [source, setSource] = useState<Source | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  // Cooling is never optimistic: "pending" only shows a spinner until the daemon confirms.
  useEffect(() => {
    if (pending && policy?.requested_preset === pending) setPending(null);
  }, [pending, policy?.requested_preset]);
  useEffect(() => () => clearTimeout(timer.current), []);

  const readings: Record<Source, SensorReading> = useMemo(
    () => ({
      cpu: readSensor(snap, SOURCE_ENTITY.cpu),
      gpu: readSensor(snap, SOURCE_ENTITY.gpu),
      liquid: readSensor(snap, SOURCE_ENTITY.liquid),
    }),
    [snap],
  );
  const available = (Object.keys(readings) as Source[]).filter((k) => readings[k].value !== null);
  const hottest = available.reduce<Source | null>(
    (best, k) =>
      best === null || (readings[k].value ?? 0) > (readings[best].value ?? 0) ? k : best,
    null,
  );
  const active: Source | null = source && available.includes(source) ? source : hottest;

  if (!policy) {
    return (
      <>
        <PageHeader title={t('cooling.title')} />
        <EmptyState title={t('cooling.policy.missing.title')}>
          {t('cooling.policy.missing.body')}
        </EmptyState>
      </>
    );
  }

  const choose = async (preset: CoolingPreset) => {
    if (preset === policy.requested_preset) return;
    setPending(preset);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setPending(null), 5000);
    const r = await actions.run(cmd.setCoolingPreset(preset));
    if (!r?.ok) setPending(null);
  };

  const effective = policy.effective_preset;
  const viewPreset = preview ?? effective;
  const curve = presetCurve(policy, viewPreset);
  const effectiveCurve = presetCurve(policy, effective);
  const ghost = preview && preview !== effective ? presetCurve(policy, preview) : null;
  const shownCurve = ghost ? effectiveCurve : curve;
  const th = policy.thresholds;
  const thresholds: CurveThreshold[] = [
    { tempC: th.cpu_critical_c, label: t('cooling.source.cpu') },
    { tempC: th.gpu_critical_c, label: t('cooling.source.gpu') },
    { tempC: th.liquid_critical_c, label: t('cooling.source.liquid') },
  ];
  const activeTemp = active ? readings[active].value : null;
  const marker =
    active && activeTemp !== null
      ? { tempC: activeTemp, label: `${t(`cooling.source.${active}`)} ${temp(activeTemp)}` }
      : null;
  const fanDuty = readSensor(snap, ENTITY.fanDuty).value;
  const pumpDuty = readSensor(snap, ENTITY.pumpDuty).value;
  const failsafe = policy.state === 'failsafe';
  const unavailable = (Object.keys(readings) as Source[]).filter((k) => readings[k].value === null);

  const onRadioKey = (e: React.KeyboardEvent, i: number) => {
    const dir =
      e.key === 'ArrowDown' || e.key === 'ArrowRight'
        ? 1
        : e.key === 'ArrowUp' || e.key === 'ArrowLeft'
          ? -1
          : 0;
    if (!dir) return;
    e.preventDefault();
    const next = COOLING_PRESETS[(i + dir + COOLING_PRESETS.length) % COOLING_PRESETS.length];
    if (next) {
      const el = document.getElementById(`preset-${next}`);
      el?.focus();
      void choose(next);
    }
  };

  return (
    <>
      <PageHeader
        title={t('cooling.title')}
        description={t('cooling.description')}
        actions={
          <Badge tone={failsafe ? 'danger' : 'success'}>
            {failsafe ? t('cooling.state.failsafe') : t('cooling.state.normal')}
          </Badge>
        }
      />

      {failsafe ? (
        <div className="mb-4">
          <Banner tone="danger" role="alert" title={t('cooling.failsafe.title')}>
            {policy.reason ? code('cooling.reason', policy.reason) : t('alert.failsafe.body')}{' '}
            {t('cooling.failsafe.body')}
          </Banner>
        </div>
      ) : null}

      <Card title={t('cooling.presets.title')} description={t('cooling.presets.description')}>
        <div
          role="radiogroup"
          aria-label={t('cooling.presets.title')}
          className="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(160px,1fr))]"
        >
          {COOLING_PRESETS.map((p, i) => {
            const selected = (pending ?? policy.requested_preset) === p;
            const pc = presetCurve(policy, p);
            return (
              <button
                key={p}
                id={`preset-${p}`}
                type="button"
                role="radio"
                aria-checked={selected}
                aria-labelledby={`preset-${p}-n`}
                aria-describedby={`preset-${p}-d`}
                tabIndex={selected ? 0 : -1}
                onClick={() => void choose(p)}
                onKeyDown={(e) => onRadioKey(e, i)}
                onPointerEnter={() => setPreview(p)}
                onPointerLeave={() => setPreview(null)}
                onFocus={() => setPreview(p)}
                onBlur={() => setPreview(null)}
                className={cn(
                  'rounded-control border p-3 text-left transition-colors',
                  selected ? 'border-accent bg-accent-soft' : 'border-line hover:bg-surface-2',
                )}
              >
                <span className="flex items-center justify-between gap-2">
                  <span id={`preset-${p}-n`} className="text-sm font-semibold text-fg">
                    {t(`preset.${p}`)}
                  </span>
                  {pending === p ? (
                    <Loader2
                      className="cm-spin size-3.5 text-muted"
                      aria-label={t('common.loading')}
                    />
                  ) : null}
                  {pending !== p && effective === p && policy.requested_preset !== p ? (
                    <Badge tone="accent">{t('cooling.applied')}</Badge>
                  ) : null}
                </span>
                <span id={`preset-${p}-d`} className="mt-1 block text-xs text-muted">
                  {t(`preset.${p}.desc`)}
                </span>
                {pc ? (
                  <span className="mt-2 block text-xs text-subtle tabular-nums">
                    {t('cooling.pump.floor', { value: pct(pc.pump_floor) })}
                  </span>
                ) : null}
              </button>
            );
          })}
        </div>
        {policy.requested_preset === 'auto' && effective !== 'auto' ? (
          <p className="mt-3 text-xs text-muted">
            {t('cooling.auto.gaming', { preset: t(`preset.${effective}`) })}
          </p>
        ) : null}
      </Card>

      <div className="mt-4">
        <Card
          title={t('cooling.curve.title', { preset: t(`preset.${viewPreset}`) })}
          description={t('cooling.curve.description')}
          actions={
            available.length > 1 ? (
              <Segmented
                label={t('cooling.source.label')}
                value={active ?? available[0] ?? 'cpu'}
                options={available.map((k) => ({ value: k, label: t(`cooling.source.${k}`) }))}
                onChange={setSource}
              />
            ) : undefined
          }
        >
          {shownCurve && shownCurve.fan_curve.length > 0 ? (
            <CurveChart
              curve={shownCurve.fan_curve}
              ghost={ghost?.fan_curve ?? null}
              pumpFloor={shownCurve.pump_floor}
              pumpFloorLabel={t('cooling.pump.floor', { value: pct(shownCurve.pump_floor) })}
              thresholds={thresholds}
              marker={marker}
              unit={unit}
              locale={locale}
              title={t('cooling.curve.title', { preset: t(`preset.${viewPreset}`) })}
              description={t('cooling.chart.desc', {
                points: shownCurve.fan_curve.length,
                pump: pct(shownCurve.pump_floor),
              })}
              tableHeaders={[t('cooling.table.temp'), t('cooling.table.duty')]}
              formatDuty={(d) => pct(d)}
              formatTemp={(c) => temp(c)}
            />
          ) : (
            <EmptyState title={t('cooling.curve.missing')} />
          )}
          <dl className="mt-3 grid gap-x-8 sm:grid-cols-2">
            <KeyValue label={t('cooling.live.fan')}>
              {fanDuty === null ? t('common.unavailable') : pct(fanDuty)}
            </KeyValue>
            <KeyValue label={t('cooling.live.pump')}>
              {pumpDuty === null ? t('common.unavailable') : pct(pumpDuty)}
            </KeyValue>
          </dl>
          {ghost ? <p className="mt-2 text-xs text-subtle">{t('cooling.preview.note')}</p> : null}
        </Card>
      </div>

      {unavailable.length > 0 ? (
        <div className="mt-4 space-y-2">
          {unavailable.map((k) => {
            const r = readings[k];
            const reason = r.reason ?? 'error';
            return (
              <Banner
                key={k}
                tone="warning"
                title={t('cooling.sensor.unavailable', { name: t(`cooling.source.${k}`) })}
                actions={
                  reason === 'requires_deep_hardware' ? (
                    <Button size="sm" onClick={() => setScreen('settings')}>
                      {t('cooling.open.settings')}
                    </Button>
                  ) : undefined
                }
              >
                {r.reason ? code('sensor', reason) : t('sensor.no_value')}
              </Banner>
            );
          })}
        </div>
      ) : null}

      <div className="mt-4">
        <Card title={t('cooling.safety.title')} description={t('cooling.safety.description')}>
          <dl className="divide-y divide-line">
            <KeyValue label={t('cooling.threshold.cpu.long')}>
              {temp(th.cpu_critical_c)}
              <span className="ml-2 text-xs font-normal text-subtle">
                {t('cooling.threshold.cpu.range', {
                  min: temp(CPU_CRITICAL_MIN_C),
                  max: temp(CPU_CRITICAL_MAX_C),
                })}
              </span>
            </KeyValue>
            <KeyValue label={t('cooling.threshold.gpu.long')}>{temp(th.gpu_critical_c)}</KeyValue>
            <KeyValue label={t('cooling.threshold.liquid.long')}>
              {temp(th.liquid_critical_c)}
            </KeyValue>
            <KeyValue label={t('cooling.threshold.hysteresis')}>
              {temp(th.hysteresis_c, { delta: true })}
            </KeyValue>
            <KeyValue label={t('cooling.threshold.pump_floor_c')}>{temp(th.pump_floor_c)}</KeyValue>
          </dl>
          <ul className="mt-4 list-disc space-y-1 pl-5 text-xs text-muted">
            <li>{t('cooling.rule.pump', { min: pct(MIN_PUMP_PERCENT) })}</li>
            <li>{t('cooling.rule.critical')}</li>
            <li>{t('cooling.rule.missing')}</li>
            <li>{t('cooling.rule.shutdown')}</li>
          </ul>
          <p className="mt-3 text-xs text-subtle">{t('cooling.thresholds.readonly')}</p>
        </Card>
      </div>
    </>
  );
}
