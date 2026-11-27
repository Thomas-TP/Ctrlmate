import { useState } from 'react';
import { useI18n } from '../../../i18n';
import { numberOf, presetCurve, readSensor } from '../../../lib/snapshot';
import { celsiusToUnit } from '../../../lib/format';
import { ENTITY, MIN_PUMP_PERCENT, type DaemonSnapshot } from '../../../types/protocol';
import { CurveChart } from '../../../ui/charts';
import { Banner, Segmented, SectionTitle } from '../../../ui/kit';
import { PresetCards, usePresetChoice } from '../../shared/controls';

type Source = 'cpu' | 'gpu' | 'liquid';
const SOURCE_ENTITY: Record<Source, string> = {
  cpu: ENTITY.cpuTemp,
  gpu: ENTITY.gpuTemp,
  liquid: ENTITY.liquidTemp,
};

export function CoolingSheet({ snap }: { snap: DaemonSnapshot }) {
  const { t, code, temp, pct, unit } = useI18n();
  const { selected } = usePresetChoice(snap);
  const [source, setSource] = useState<Source>('liquid');
  const policy = snap.cooling_policy;

  if (!policy) {
    return (
      <Banner tone="warning" title={t('cooling.policy.missing.title')}>
        {t('cooling.policy.missing.body')}
      </Banner>
    );
  }
  const curve = presetCurve(policy, policy.effective_preset);
  const reading = readSensor(snap, SOURCE_ENTITY[source]);
  const duty = numberOf(snap, ENTITY.fanDuty);
  const failsafe = policy.state === 'failsafe';
  const th = policy.thresholds;
  const critical =
    source === 'cpu'
      ? th.cpu_critical_c
      : source === 'gpu'
        ? th.gpu_critical_c
        : th.liquid_critical_c;
  const suffix = unit === 'fahrenheit' ? '°F' : '°';

  return (
    <div>
      {failsafe ? (
        <div className="mb-5">
          <Banner tone="danger" role="alert" title={t('cooling.failsafe.title')}>
            {policy.reason ? `${code('cooling.reason', policy.reason)} ` : ''}
            {t('cooling.failsafe.body')}
          </Banner>
        </div>
      ) : null}

      <SectionTitle>{t('cooling.presets.title')}</SectionTitle>
      <PresetCards snap={snap} />
      {selected === 'auto' && snap.session?.gaming_mode ? (
        <p className="mt-2 text-[13px] text-muted">
          {t('cooling.auto.gaming', { preset: t(`preset.${policy.effective_preset}`) })}
        </p>
      ) : null}

      <SectionTitle
        aside={
          <Segmented
            size="sm"
            label={t('cooling.source.label')}
            value={source}
            onChange={setSource}
            options={[
              { value: 'cpu', label: t('cooling.source.cpu') },
              { value: 'gpu', label: t('cooling.source.gpu') },
              { value: 'liquid', label: t('cooling.source.liquid') },
            ]}
          />
        }
      >
        {t('cooling.curve.title', { preset: t(`preset.${policy.effective_preset}`) })}
      </SectionTitle>
      {curve ? (
        <div className="rounded-xl border border-line p-3">
          <CurveChart
            curve={curve.fan_curve}
            marker={reading.value !== null && duty !== null ? { temp: reading.value, duty } : null}
            criticalC={critical}
            label={t('cooling.chart.desc')}
            unitSuffix={suffix}
            toUnit={(c) => celsiusToUnit(c, unit)}
          />
          <p className="mt-1 px-1 text-[13px] text-muted tabular">
            {reading.value !== null
              ? `${temp(reading.value)} → ${pct(duty)}`
              : t('cooling.sensor.unavailable')}
            {' · '}
            {t('cooling.pump.floor', { value: pct(curve.pump_floor) })}
          </p>
        </div>
      ) : (
        <Banner tone="neutral">{t('cooling.curve.missing')}</Banner>
      )}

      <SectionTitle>{t('cooling.safety.title')}</SectionTitle>
      <ul className="space-y-2 text-[13px] text-muted">
        <li>{t('cooling.rule.pump', { min: pct(MIN_PUMP_PERCENT) })}</li>
        <li>{t('cooling.rule.critical')}</li>
        <li>{t('cooling.rule.missing')}</li>
        <li>{t('cooling.rule.shutdown')}</li>
      </ul>
      <dl className="mt-3 divide-y divide-line rounded-xl border border-line px-4 text-sm">
        {(
          [
            ['cooling.threshold.cpu.long', th.cpu_critical_c],
            ['cooling.threshold.gpu.long', th.gpu_critical_c],
            ['cooling.threshold.liquid.long', th.liquid_critical_c],
          ] as const
        ).map(([key, c]) => (
          <div key={key} className="flex justify-between py-2">
            <dt className="text-muted">{t(key)}</dt>
            <dd className="font-medium text-fg tabular">{temp(c)}</dd>
          </div>
        ))}
      </dl>
      <p className="mt-2 text-xs text-subtle">{t('cooling.thresholds.readonly')}</p>
    </div>
  );
}
