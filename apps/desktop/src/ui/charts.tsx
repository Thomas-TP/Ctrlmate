import { useId } from 'react';
import type { FanCurvePoint } from '../types/protocol';
import type { HistoryPoint } from '../store/reducers';

/** Smooth area sparkline. Purely decorative unless `label` is given (then it is an image). */
export function Sparkline({
  points,
  height = 56,
  label,
  min,
  max,
}: {
  points: readonly HistoryPoint[];
  height?: number;
  label?: string;
  min?: number;
  max?: number;
}) {
  const gid = useId();
  const w = 300;
  if (points.length < 2) {
    return (
      <svg viewBox={`0 0 ${w} ${height}`} className="w-full" style={{ height }} aria-hidden>
        <line
          x1="0"
          x2={w}
          y1={height - 2}
          y2={height - 2}
          stroke="var(--chart-grid)"
          strokeWidth="2"
          strokeDasharray="4 6"
        />
      </svg>
    );
  }
  const values = points.map((p) => p.v);
  const lo = min ?? Math.min(...values) - 2;
  const hi = max ?? Math.max(...values) + 2;
  const span = Math.max(hi - lo, 1);
  const pad = 4;
  const xs = (i: number) => (i / (points.length - 1)) * w;
  const ys = (v: number) => height - pad - ((v - lo) / span) * (height - pad * 2);
  const line = points
    .map((p, i) => `${i === 0 ? 'M' : 'L'}${xs(i).toFixed(1)} ${ys(p.v).toFixed(1)}`)
    .join(' ');
  const area = `${line} L${w} ${height} L0 ${height} Z`;
  return (
    <svg
      viewBox={`0 0 ${w} ${height}`}
      preserveAspectRatio="none"
      className="w-full"
      style={{ height }}
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <defs>
        <linearGradient id={gid} x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor="var(--chart-line)" stopOpacity="0.22" />
          <stop offset="1" stopColor="var(--chart-line)" stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={area} fill={`url(#${gid})`} />
      <path
        d={line}
        fill="none"
        stroke="var(--chart-line)"
        strokeWidth="2"
        strokeLinejoin="round"
        strokeLinecap="round"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

/** Fan duty versus temperature, with the current operating point. */
export function CurveChart({
  curve,
  marker,
  criticalC,
  label,
  unitSuffix,
  toUnit,
}: {
  curve: readonly FanCurvePoint[];
  marker?: { temp: number; duty: number } | null;
  criticalC?: number;
  label: string;
  unitSuffix: string;
  toUnit: (c: number) => number;
}) {
  const W = 480;
  const H = 180;
  const L = 34;
  const B = 24;
  const T = 8;
  const R = 8;
  const tMin = 20;
  const tMax = 100;
  const x = (c: number) =>
    L + ((Math.min(Math.max(c, tMin), tMax) - tMin) / (tMax - tMin)) * (W - L - R);
  const y = (d: number) => T + (1 - Math.min(Math.max(d, 0), 100) / 100) * (H - T - B);
  const pts = [...curve].sort((a, b) => a.temp_c - b.temp_c);
  const path = pts
    .map((p, i) => `${i === 0 ? 'M' : 'L'}${x(p.temp_c).toFixed(1)} ${y(p.duty).toFixed(1)}`)
    .join(' ');
  const xTicks = [20, 40, 60, 80, 100];
  return (
    <svg viewBox={`0 0 ${W} ${H}`} className="w-full" role="img" aria-label={label}>
      {[0, 50, 100].map((d) => (
        <g key={d}>
          <line x1={L} x2={W - R} y1={y(d)} y2={y(d)} stroke="var(--chart-grid)" />
          <text x={L - 6} y={y(d) + 4} textAnchor="end" fontSize="10" fill="var(--fg-subtle)">
            {d}%
          </text>
        </g>
      ))}
      {xTicks.map((c) => (
        <text key={c} x={x(c)} y={H - 6} textAnchor="middle" fontSize="10" fill="var(--fg-subtle)">
          {Math.round(toUnit(c))}
          {unitSuffix}
        </text>
      ))}
      {criticalC !== undefined ? (
        <line
          x1={x(criticalC)}
          x2={x(criticalC)}
          y1={T}
          y2={H - B}
          stroke="var(--danger)"
          strokeDasharray="4 4"
          opacity="0.6"
        />
      ) : null}
      <path
        d={path}
        fill="none"
        stroke="var(--chart-line)"
        strokeWidth="2.5"
        strokeLinejoin="round"
      />
      {pts.map((p) => (
        <circle
          key={p.temp_c}
          cx={x(p.temp_c)}
          cy={y(p.duty)}
          r="3"
          fill="var(--surface)"
          stroke="var(--chart-line)"
          strokeWidth="2"
        />
      ))}
      {marker ? (
        <g>
          <line
            x1={x(marker.temp)}
            x2={x(marker.temp)}
            y1={T}
            y2={H - B}
            stroke="var(--chart-ghost)"
            strokeDasharray="2 4"
          />
          <circle
            cx={x(marker.temp)}
            cy={y(marker.duty)}
            r="6"
            fill="var(--chart-line)"
            stroke="var(--surface)"
            strokeWidth="2"
          />
        </g>
      ) : null}
    </svg>
  );
}
