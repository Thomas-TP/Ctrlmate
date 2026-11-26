import type { ReactNode } from 'react';
import { useI18n } from '../i18n';
import type { SensorReading } from '../lib/snapshot';
import type { HistoryPoint } from '../store/reducers';
import { Sparkline } from './charts/Sparkline';

interface MetricCardProps {
  label: string;
  /** Formatted value, or `null` when the sensor has no value. */
  value: string | null;
  /** Second line (e.g. rpm). */
  secondary?: string | null;
  reading?: SensorReading;
  history: readonly HistoryPoint[];
  format: (v: number) => string;
  domain?: readonly [number, number];
  /** Tone of the value when it is critical. */
  alert?: boolean;
  footer?: ReactNode;
}

/** One live metric: big tabular number + sparkline + honest unavailable state. */
export function MetricCard({
  label,
  value,
  secondary,
  reading,
  history,
  format,
  domain,
  alert,
  footer,
}: MetricCardProps) {
  const { t, code } = useI18n();
  const values = history.map((h) => h.v);
  const min = values.length ? Math.min(...values) : null;
  const max = values.length ? Math.max(...values) : null;
  const span =
    history.length >= 2
      ? Math.max(
          1,
          Math.round(((history[history.length - 1]?.t ?? 0) - (history[0]?.t ?? 0)) / 60000),
        )
      : 0;
  const summary =
    value !== null && min !== null && max !== null && span > 0
      ? t('metric.sparkline.summary', {
          name: label,
          minutes: span,
          min: format(min),
          max: format(max),
          now: value,
        })
      : value !== null
        ? t('metric.sparkline.now', { name: label, now: value })
        : t('metric.sparkline.none', { name: label });
  return (
    <section aria-label={label} className="rounded-card border border-line bg-surface p-4">
      <div className="flex items-start justify-between gap-2">
        <h3 className="text-xs font-medium text-muted">{label}</h3>
        {value !== null ? <Sparkline values={values} summary={summary} domain={domain} /> : null}
      </div>
      <p
        className={`mt-1 text-2xl font-semibold tracking-tight tabular-nums ${alert ? 'text-danger' : 'text-fg'}`}
      >
        {value ?? t('common.unavailable')}
      </p>
      {value !== null ? (
        secondary ? (
          <p className="mt-0.5 text-xs text-muted tabular-nums">{secondary}</p>
        ) : null
      ) : (
        <p className="mt-0.5 text-xs text-muted">
          {reading?.reason === 'requires_deep_hardware'
            ? t('sensor.requires_deep_hardware.short')
            : reading?.reason
              ? code('sensor', reading.reason)
              : t('sensor.no_value')}
        </p>
      )}
      {footer}
    </section>
  );
}
