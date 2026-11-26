import { useEffect, useId, useRef, useState } from 'react';
import { formatTemperatureTick } from '../../lib/format';
import type { FanCurvePoint, TemperatureUnit } from '../../types/protocol';

export interface CurveMarker {
  tempC: number;
  label: string;
}

export interface CurveThreshold {
  tempC: number;
  label: string;
}

interface CurveChartProps {
  curve: readonly FanCurvePoint[];
  /** Curve of another preset, drawn dashed behind (hover / focus preview). */
  ghost?: readonly FanCurvePoint[] | null;
  pumpFloor: number | null;
  pumpFloorLabel: string;
  thresholds: readonly CurveThreshold[];
  marker: CurveMarker | null;
  unit: TemperatureUnit;
  locale: string;
  /** Title + description for assistive tech. */
  title: string;
  description: string;
  tableHeaders: readonly [string, string];
  /** Formats a duty value for the text table / axis ("45 %"). */
  formatDuty: (duty: number) => string;
  /** Formats a temperature for the text table ("62 °C"). */
  formatTemp: (celsius: number) => string;
  height?: number;
}

/** Linear interpolation on the policy curve; clamped to the first / last point. */
export function interpolateDuty(curve: readonly FanCurvePoint[], tempC: number): number | null {
  const first = curve[0];
  const last = curve[curve.length - 1];
  if (!first || !last) return null;
  if (tempC <= first.temp_c) return first.duty;
  if (tempC >= last.temp_c) return last.duty;
  for (let i = 1; i < curve.length; i++) {
    const a = curve[i - 1];
    const b = curve[i];
    if (a && b && tempC <= b.temp_c) {
      const f = (tempC - a.temp_c) / (b.temp_c - a.temp_c);
      return a.duty + f * (b.duty - a.duty);
    }
  }
  return last.duty;
}

function useWidth(): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(640);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w && w > 0) setWidth(Math.floor(w));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}

/** Fan-curve chart drawn in real pixels so text never drops below 12 px on narrow windows. */
export function CurveChart(p: CurveChartProps) {
  const [ref, width] = useWidth();
  const id = useId();
  const height = p.height ?? 280;
  const m = { top: 46, right: 16, bottom: 34, left: 44 };
  const iw = Math.max(120, width - m.left - m.right);
  const ih = height - m.top - m.bottom;

  const temps = [
    ...p.curve.map((c) => c.temp_c),
    ...(p.ghost?.map((c) => c.temp_c) ?? []),
    ...p.thresholds.map((t) => t.tempC),
    ...(p.marker ? [p.marker.tempC] : []),
  ];
  const lo = Math.floor((Math.min(...(temps.length ? temps : [30])) - 5) / 10) * 10;
  const hi = Math.ceil((Math.max(...(temps.length ? temps : [90])) + 2) / 10) * 10;
  const x = (t: number) => m.left + ((t - lo) / (hi - lo)) * iw;
  const y = (d: number) => m.top + (1 - d / 100) * ih;
  const path = (pts: readonly FanCurvePoint[]) =>
    pts
      .map((pt, i) => `${i ? 'L' : 'M'}${x(pt.temp_c).toFixed(1)} ${y(pt.duty).toFixed(1)}`)
      .join(' ');

  const xTicks: number[] = [];
  const step = iw < 360 ? 20 : 10;
  for (let t = Math.ceil(lo / step) * step; t <= hi; t += step) xTicks.push(t);
  const yTicks = [0, 25, 50, 75, 100];
  const markerDuty = p.marker ? interpolateDuty(p.curve, p.marker.tempC) : null;

  return (
    <figure ref={ref} className="m-0 w-full">
      <svg
        width={width}
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-labelledby={`${id}-t ${id}-d`}
        className="block"
      >
        <title id={`${id}-t`}>{p.title}</title>
        <desc id={`${id}-d`}>{p.description}</desc>

        {yTicks.map((d) => (
          <g key={`y${d}`}>
            <line x1={m.left} x2={m.left + iw} y1={y(d)} y2={y(d)} stroke="var(--chart-grid)" />
            <text
              x={m.left - 8}
              y={y(d) + 4}
              textAnchor="end"
              fontSize={12}
              fill="var(--fg-subtle)"
              className="tabular-nums"
            >
              {d}
            </text>
          </g>
        ))}
        {xTicks.map((t) => (
          <text
            key={`x${t}`}
            x={x(t)}
            y={height - 12}
            textAnchor="middle"
            fontSize={12}
            fill="var(--fg-subtle)"
            className="tabular-nums"
          >
            {formatTemperatureTick(t, p.unit, p.locale)}
          </text>
        ))}

        {p.pumpFloor !== null ? (
          <g>
            <line
              x1={m.left}
              x2={m.left + iw}
              y1={y(p.pumpFloor)}
              y2={y(p.pumpFloor)}
              stroke="var(--fg-subtle)"
              strokeDasharray="2 4"
            />
            <text
              x={m.left + iw - 4}
              y={y(p.pumpFloor) - 6}
              textAnchor="end"
              fontSize={12}
              fill="var(--fg-subtle)"
            >
              {p.pumpFloorLabel}
            </text>
          </g>
        ) : null}

        {p.thresholds.map((t, i) => (
          <g key={`th${t.label}${i}`}>
            <line
              x1={x(t.tempC)}
              x2={x(t.tempC)}
              y1={m.top}
              y2={m.top + ih}
              stroke="var(--danger)"
              strokeDasharray="4 4"
              opacity={0.7}
            />
            <text
              x={x(t.tempC)}
              y={m.top - 10 - (i % 2) * 15}
              textAnchor="middle"
              fontSize={12}
              fill="var(--danger)"
            >
              {t.label}
            </text>
          </g>
        ))}

        {p.ghost && p.ghost.length > 1 ? (
          <path
            d={path(p.ghost)}
            fill="none"
            stroke="var(--chart-ghost)"
            strokeWidth={1.5}
            strokeDasharray="5 4"
          />
        ) : null}

        {p.curve.length > 1 ? (
          <path
            d={path(p.curve)}
            fill="none"
            stroke="var(--chart-line)"
            strokeWidth={2.25}
            strokeLinejoin="round"
            strokeLinecap="round"
          />
        ) : null}
        {p.curve.map((pt) => (
          <circle
            key={pt.temp_c}
            cx={x(pt.temp_c)}
            cy={y(pt.duty)}
            r={3.5}
            fill="var(--surface)"
            stroke="var(--chart-line)"
            strokeWidth={1.75}
          />
        ))}

        {p.marker && markerDuty !== null ? (
          <g>
            <line
              x1={x(p.marker.tempC)}
              x2={x(p.marker.tempC)}
              y1={y(markerDuty)}
              y2={m.top + ih}
              stroke="var(--fg)"
              strokeDasharray="2 3"
              opacity={0.5}
            />
            <circle
              cx={x(p.marker.tempC)}
              cy={y(markerDuty)}
              r={6}
              fill="var(--fg)"
              stroke="var(--surface)"
              strokeWidth={2}
            />
            <text
              x={Math.min(x(p.marker.tempC) + 10, m.left + iw - 4)}
              y={Math.max(y(markerDuty) - 10, m.top + 10)}
              textAnchor={x(p.marker.tempC) + 10 > m.left + iw - 80 ? 'end' : 'start'}
              fontSize={12}
              fontWeight={600}
              fill="var(--fg)"
              stroke="var(--surface)"
              strokeWidth={4}
              paintOrder="stroke"
            >
              {p.marker.label}
            </text>
          </g>
        ) : null}
      </svg>
      <figcaption className="sr-only">
        <table>
          <caption>{p.title}</caption>
          <thead>
            <tr>
              <th scope="col">{p.tableHeaders[0]}</th>
              <th scope="col">{p.tableHeaders[1]}</th>
            </tr>
          </thead>
          <tbody>
            {p.curve.map((pt) => (
              <tr key={pt.temp_c}>
                <td>{p.formatTemp(pt.temp_c)}</td>
                <td>{p.formatDuty(pt.duty)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </figcaption>
    </figure>
  );
}
