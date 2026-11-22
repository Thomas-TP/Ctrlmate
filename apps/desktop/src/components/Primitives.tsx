import React, { useId } from 'react';

export function CtrlLogo({ className = 'h-4 w-4' }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" className={className} aria-hidden="true">
      <rect x="2.5" y="2.5" width="19" height="19" rx="5.5" stroke="currentColor" strokeWidth="1.8" />
      <path
        d="M15.6 9.3a4.2 4.2 0 1 0 0 5.4"
        stroke="currentColor"
        strokeWidth="1.9"
        strokeLinecap="round"
      />
      <circle cx="15.8" cy="12" r="1.4" fill="currentColor" />
    </svg>
  );
}

export function SurfaceCard({
  title,
  subtitle,
  action,
  children,
  className = '',
  noPadding = false,
}: {
  title?: string;
  subtitle?: string;
  action?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
  noPadding?: boolean;
}) {
  return (
    <section className={`surface-card rounded-xl ${noPadding ? '' : 'p-5'} ${className}`}>
      {(title || action) && (
        <div
          className={`flex items-center justify-between gap-3 ${
            noPadding ? 'px-5 pt-4 pb-3' : 'mb-4'
          }`}
        >
          <div className="min-w-0">
            {title && (
              <h2 className="text-[11px] font-medium tracking-[0.06em] text-[var(--text-muted)] uppercase">
                {title}
              </h2>
            )}
            {subtitle && (
              <p className="mt-0.5 text-xs text-[var(--text-secondary)]">{subtitle}</p>
            )}
          </div>
          {action && <div className="shrink-0">{action}</div>}
        </div>
      )}
      {children}
    </section>
  );
}

export type BadgeTone = 'emerald' | 'amber' | 'red' | 'blue' | 'neutral';

const TONE_STYLES: Record<BadgeTone, { pill: string; dot: string }> = {
  emerald: {
    pill: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 ring-1 ring-emerald-500/20',
    dot: 'bg-emerald-500',
  },
  amber: {
    pill: 'bg-amber-500/10 text-amber-700 dark:text-amber-300 ring-1 ring-amber-500/20',
    dot: 'bg-amber-500',
  },
  red: {
    pill: 'bg-red-500/10 text-red-600 dark:text-red-400 ring-1 ring-red-500/20',
    dot: 'bg-red-500',
  },
  blue: {
    pill: 'bg-blue-500/10 text-blue-600 dark:text-blue-400 ring-1 ring-blue-500/20',
    dot: 'bg-blue-500',
  },
  neutral: {
    pill: 'bg-black/[0.04] text-[var(--text-secondary)] dark:bg-white/[0.05] ring-1 ring-black/[0.06] dark:ring-white/[0.08]',
    dot: 'bg-neutral-400 dark:bg-neutral-500',
  },
};

export function StatusBadge({
  tone = 'neutral',
  dot = true,
  pulse = false,
  children,
}: {
  tone?: BadgeTone;
  dot?: boolean;
  pulse?: boolean;
  children: React.ReactNode;
}) {
  const st = TONE_STYLES[tone];
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-[11px] font-medium ${st.pill}`}
    >
      {dot && (
        <span className="relative flex h-1.5 w-1.5">
          {pulse && (
            <span
              className={`absolute inline-flex h-full w-full animate-ping rounded-full opacity-55 ${st.dot}`}
            />
          )}
          <span className={`relative inline-flex h-1.5 w-1.5 rounded-full ${st.dot}`} />
        </span>
      )}
      {children}
    </span>
  );
}

export function ToggleSwitch({
  checked,
  onChange,
  disabled = false,
  label,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-40 ${
        checked
          ? 'bg-neutral-900 dark:bg-white'
          : 'bg-neutral-300 dark:bg-white/[0.14]'
      }`}
    >
      <span
        className={`pointer-events-none inline-block h-3.5 w-3.5 rounded-full shadow-sm transition-transform duration-150 ${
          checked
            ? 'translate-x-[18px] bg-white dark:bg-neutral-950'
            : 'translate-x-[3px] bg-white dark:bg-neutral-300'
        }`}
      />
    </button>
  );
}

/**
 * Architectural micro-sparkline (borderless, smooth SVG line + soft vertical fade + live end-dot).
 */
export function Sparkline({
  data,
  min,
  max,
  stroke = 'currentColor',
  height = 38,
  className = '',
}: {
  data: number[];
  min?: number;
  max?: number;
  stroke?: string;
  height?: number;
  className?: string;
}) {
  const gradId = useId();
  const width = 220;

  if (data.length < 2) {
    return (
      <div
        className={`flex items-center ${className}`}
        style={{ height }}
      >
        <div className="h-[1px] w-full bg-black/[0.06] dark:bg-white/[0.06]" />
      </div>
    );
  }

  const dataMin = Math.min(...data);
  const dataMax = Math.max(...data);
  const lo = min !== undefined ? Math.min(min, dataMin) : Math.max(0, dataMin - 5);
  const hi = max !== undefined ? Math.max(max, dataMax) : dataMax + 5;
  const range = Math.max(hi - lo, 1);

  const padY = 4;
  const usableH = height - padY * 2;
  const pts = data.map((v, i) => {
    const x = (i / Math.max(data.length - 1, 1)) * (width - 6) + 2;
    const y = padY + usableH - ((v - lo) / range) * usableH;
    return [x, y] as const;
  });

  // Smooth quadratic midpoint path
  let linePath = `M ${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
  for (let i = 1; i < pts.length; i++) {
    const [x0, y0] = pts[i - 1];
    const [x1, y1] = pts[i];
    const cx = ((x0 + x1) / 2).toFixed(1);
    const cy = ((y0 + y1) / 2).toFixed(1);
    linePath += ` Q ${x0.toFixed(1)} ${y0.toFixed(1)}, ${cx} ${cy}`;
  }
  const last = pts[pts.length - 1];
  linePath += ` L ${last[0].toFixed(1)} ${last[1].toFixed(1)}`;

  const areaPath = `${linePath} L ${last[0].toFixed(1)} ${height} L ${pts[0][0].toFixed(1)} ${height} Z`;

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      className={`w-full overflow-visible ${className}`}
      style={{ height }}
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={stroke} stopOpacity="0.16" />
          <stop offset="100%" stopColor={stroke} stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={areaPath} fill={`url(#${gradId})`} />
      <path
        d={linePath}
        fill="none"
        stroke={stroke}
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
        vectorEffect="non-scaling-stroke"
      />
      <circle cx={last[0]} cy={last[1]} r="2.6" fill={stroke} />
    </svg>
  );
}

/**
 * Thin architectural bar with calm thermal ramp (neutral -> amber -> brick red, zero neon).
 */
export function ThermalBar({
  percent,
  warnAt = 75,
  critAt = 90,
}: {
  percent: number;
  warnAt?: number;
  critAt?: number;
}) {
  const clamped = Math.max(0, Math.min(100, percent));
  const tone =
    clamped >= critAt
      ? 'bg-red-500/85'
      : clamped >= warnAt
        ? 'bg-amber-500/85'
        : 'bg-neutral-800 dark:bg-neutral-200';

  return (
    <div className="h-1 w-full overflow-hidden rounded-full bg-black/[0.06] dark:bg-white/[0.07]">
      <div
        className={`h-full rounded-full transition-all duration-300 ${tone}`}
        style={{ width: `${clamped}%` }}
      />
    </div>
  );
}
