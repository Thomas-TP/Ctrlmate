import { useId } from 'react';

interface SparklineProps {
  values: readonly number[];
  /** Text alternative, e.g. "CPU temperature, last 3 minutes: min 38 °C, max 64 °C, now 52 °C". */
  summary: string;
  width?: number;
  height?: number;
  /** Fixed domain (e.g. 0-100 for loads); default: data range with padding. */
  domain?: readonly [number, number];
}

/** Minimal inline trend line. Decorative for sighted users; `summary` is the text alternative. */
export function Sparkline({ values, summary, width = 120, height = 32, domain }: SparklineProps) {
  const id = useId();
  if (values.length < 2) {
    return (
      <svg
        role="img"
        aria-label={summary}
        width={width}
        height={height}
        viewBox={`0 0 ${width} ${height}`}
      >
        <line
          x1={0}
          x2={width}
          y1={height - 1}
          y2={height - 1}
          stroke="var(--line-strong)"
          strokeDasharray="3 3"
        />
      </svg>
    );
  }
  const lo = domain ? domain[0] : Math.min(...values);
  const hi = domain ? domain[1] : Math.max(...values);
  const span = hi - lo || 1;
  const pad = 2;
  const x = (i: number) => (i / (values.length - 1)) * (width - pad * 2) + pad;
  const y = (v: number) => height - pad - ((v - lo) / span) * (height - pad * 2);
  const line = values
    .map((v, i) => `${i === 0 ? 'M' : 'L'}${x(i).toFixed(1)} ${y(v).toFixed(1)}`)
    .join(' ');
  const area = `${line} L${x(values.length - 1).toFixed(1)} ${height} L${x(0).toFixed(1)} ${height} Z`;
  const lastV = values[values.length - 1] ?? lo;
  return (
    <svg
      role="img"
      aria-label={summary}
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
    >
      <title id={id}>{summary}</title>
      <path d={area} fill="var(--chart-line)" opacity={0.1} />
      <path
        d={line}
        fill="none"
        stroke="var(--chart-line)"
        strokeWidth={1.5}
        strokeLinejoin="round"
        strokeLinecap="round"
      />
      <circle cx={x(values.length - 1)} cy={y(lastV)} r={2.5} fill="var(--chart-line)" />
    </svg>
  );
}
