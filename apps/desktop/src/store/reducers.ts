import type { ConnectionStatus } from '../api/app';
import { numberOf } from '../lib/snapshot';
import { ENTITY, type DaemonSnapshot } from '../types/protocol';

/** Metrics with client-side history (sparklines). */
export const HISTORY_METRICS = {
  cpuTemp: ENTITY.cpuTemp,
  gpuTemp: ENTITY.gpuTemp,
  liquidTemp: ENTITY.liquidTemp,
  cpuLoad: ENTITY.cpuLoad,
  gpuLoad: ENTITY.gpuLoad,
  fanDuty: ENTITY.fanDuty,
  pumpDuty: ENTITY.pumpDuty,
} as const;
export type HistoryMetric = keyof typeof HISTORY_METRICS;

export interface HistoryPoint {
  t: number;
  v: number;
}
export type History = Record<HistoryMetric, HistoryPoint[]>;

/** Minimum spacing between two stored samples, and the ring-buffer size (about 3 minutes). */
export const HISTORY_MIN_INTERVAL_MS = 1500;
export const HISTORY_MAX_POINTS = 120;

export function emptyHistory(): History {
  return {
    cpuTemp: [],
    gpuTemp: [],
    liquidTemp: [],
    cpuLoad: [],
    gpuLoad: [],
    fanDuty: [],
    pumpDuty: [],
  };
}

export interface DaemonView {
  snapshot: DaemonSnapshot | null;
  /** Epoch ms of the last snapshot. */
  snapshotAt: number | null;
  connection: ConnectionStatus & { since: number };
  history: History;
}

export function initialView(now: number): DaemonView {
  return {
    snapshot: null,
    snapshotAt: null,
    connection: { status: 'connecting', reason: null, attempt: 0, since: now },
    history: emptyHistory(),
  };
}

/** Appends a sample unless the previous one is too recent; missing sensors add nothing. */
export function pushSample(
  series: HistoryPoint[],
  value: number | null,
  now: number,
): HistoryPoint[] {
  if (value === null) return series;
  const last = series[series.length - 1];
  if (last && now - last.t < HISTORY_MIN_INTERVAL_MS) return series;
  const next = series.length >= HISTORY_MAX_POINTS ? series.slice(1) : series.slice();
  next.push({ t: now, v: value });
  return next;
}

export function reduceSnapshot(
  view: DaemonView,
  snapshot: DaemonSnapshot,
  now: number,
): DaemonView {
  const history = { ...view.history };
  for (const key of Object.keys(HISTORY_METRICS) as HistoryMetric[]) {
    history[key] = pushSample(view.history[key], numberOf(snapshot, HISTORY_METRICS[key]), now);
  }
  return { ...view, snapshot, snapshotAt: now, history };
}

export function reduceConnection(
  view: DaemonView,
  status: ConnectionStatus,
  now: number,
): DaemonView {
  const changed = view.connection.status !== status.status;
  return {
    ...view,
    connection: { ...status, since: changed ? now : view.connection.since },
  };
}
