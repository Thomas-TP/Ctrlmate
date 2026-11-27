import { useI18n } from '../../../i18n';
import { temperatureRows, tempFill } from '../../../lib/derive';
import { numberOf } from '../../../lib/snapshot';
import { useDaemon } from '../../../store/daemon';
import type { HistoryMetric } from '../../../store/reducers';
import { ENTITY, type DaemonSnapshot } from '../../../types/protocol';
import { Sparkline } from '../../../ui/charts';
import { SectionTitle } from '../../../ui/kit';

const SERIES: Record<string, HistoryMetric> = {
  [ENTITY.cpuTemp]: 'cpuTemp',
  [ENTITY.gpuTemp]: 'gpuTemp',
  [ENTITY.liquidTemp]: 'liquidTemp',
};

export function TempsSheet({ snap }: { snap: DaemonSnapshot }) {
  const { t, temp, pct, code } = useI18n();
  const history = useDaemon((s) => s.history);
  const loads = [
    { key: 'metric.cpu_load', v: numberOf(snap, ENTITY.cpuLoad) },
    { key: 'metric.gpu_load', v: numberOf(snap, ENTITY.gpuLoad) },
    { key: 'dash.memory', v: numberOf(snap, ENTITY.memoryUsage) },
  ] as const;
  return (
    <div>
      <SectionTitle>{t('dash.temps.title')}</SectionTitle>
      <ul className="space-y-3">
        {temperatureRows(snap).map((r) => {
          const metric = SERIES[r.id];
          return (
            <li key={r.id} className="rounded-xl border border-line p-4">
              <div className="flex items-baseline justify-between">
                <span className="text-sm font-medium text-fg">{r.name}</span>
                {r.reading.value !== null ? (
                  <span className="text-xl font-semibold text-fg tabular">
                    {temp(r.reading.value)}
                  </span>
                ) : (
                  <span className="text-[13px] text-subtle">
                    {r.reading.reason ? code('sensor', r.reading.reason) : t('common.unavailable')}
                  </span>
                )}
              </div>
              {metric && history[metric].length > 1 ? (
                <div className="mt-2">
                  <Sparkline
                    points={history[metric]}
                    height={48}
                    min={20}
                    max={100}
                    label={t('metric.sparkline.now', { name: r.name, now: temp(r.reading.value) })}
                  />
                </div>
              ) : r.reading.value !== null ? (
                <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-surface-2">
                  <div
                    className="h-full rounded-full bg-accent"
                    style={{ width: `${tempFill(r.reading.value)}%` }}
                  />
                </div>
              ) : null}
            </li>
          );
        })}
      </ul>

      <SectionTitle>{t('dash.load.title')}</SectionTitle>
      <ul className="space-y-3">
        {loads.map(({ key, v }) =>
          v === null ? null : (
            <li key={key}>
              <div className="mb-1 flex justify-between text-sm">
                <span className="text-muted">{t(key)}</span>
                <span className="font-medium text-fg tabular">{pct(v)}</span>
              </div>
              <div className="h-1.5 overflow-hidden rounded-full bg-surface-2">
                <div
                  className="h-full rounded-full bg-accent"
                  style={{ width: `${Math.min(100, v)}%` }}
                />
              </div>
            </li>
          ),
        )}
      </ul>
    </div>
  );
}
