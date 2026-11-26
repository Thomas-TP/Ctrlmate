import type { TemperatureUnit } from '../types/protocol';

/** Placeholder shown for any value the daemon could not measure (never a fabricated number). */
export const EM_DASH = '—';

export function celsiusToUnit(celsius: number, unit: TemperatureUnit): number {
  return unit === 'fahrenheit' ? (celsius * 9) / 5 + 32 : celsius;
}

export function unitToCelsius(value: number, unit: TemperatureUnit): number {
  return unit === 'fahrenheit' ? ((value - 32) * 5) / 9 : value;
}

export interface TempFormatOptions {
  digits?: number;
  /** Append the unit symbol (default true). */
  withUnit?: boolean;
  /** Treat the value as a temperature *difference* (no 32 degree offset). */
  delta?: boolean;
}

/**
 * The one temperature formatter of the app (cards, charts, axes, thresholds).
 * The protocol is always Celsius; conversion happens only here.
 */
export function formatTemperature(
  celsius: number | null | undefined,
  unit: TemperatureUnit,
  locale: string,
  opts: TempFormatOptions = {},
): string {
  if (celsius === null || celsius === undefined || !Number.isFinite(celsius)) return EM_DASH;
  const { digits = 0, withUnit = true, delta = false } = opts;
  const converted = delta
    ? unit === 'fahrenheit'
      ? (celsius * 9) / 5
      : celsius
    : celsiusToUnit(celsius, unit);
  const fmt = new Intl.NumberFormat(locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
    ...(withUnit
      ? { style: 'unit' as const, unit: unit === 'fahrenheit' ? 'fahrenheit' : 'celsius' }
      : {}),
  });
  return fmt.format(converted);
}

/** Axis tick: number only, in the display unit. */
export function formatTemperatureTick(
  celsius: number,
  unit: TemperatureUnit,
  locale: string,
): string {
  return formatTemperature(celsius, unit, locale, { withUnit: false });
}

export function formatNumber(value: number | null | undefined, locale: string, digits = 0): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return EM_DASH;
  return new Intl.NumberFormat(locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(value);
}

export function formatPercent(
  value: number | null | undefined,
  locale: string,
  digits = 0,
): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return EM_DASH;
  return new Intl.NumberFormat(locale, {
    style: 'unit',
    unit: 'percent',
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(value);
}

export function formatRpm(value: number | null | undefined, locale: string): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return EM_DASH;
  return `${formatNumber(value, locale)}\u202f${locale.startsWith('fr') ? 'tr/min' : 'rpm'}`;
}

export function formatKelvin(value: number, locale: string): string {
  return `${formatNumber(value, locale)}\u202fK`;
}

export function formatDateTime(iso: string | null | undefined, locale: string): string {
  if (!iso) return EM_DASH;
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return EM_DASH;
  return new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short' }).format(d);
}

/** `mm:ss` countdown for the pairing PIN. Negative values clamp to `00:00`. */
export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
}

/** Splits a hex fingerprint into groups (default 4 chars), upper-cased, for visual comparison. */
export function groupFingerprint(hex: string, group = 4): string[] {
  const clean = hex.replace(/[^0-9a-fA-F]/g, '').toUpperCase();
  const out: string[] = [];
  for (let i = 0; i < clean.length; i += group) out.push(clean.slice(i, i + group));
  return out;
}

/** `now - then` as a localised relative phrase ("5 minutes ago"). */
export function formatRelative(
  iso: string | null | undefined,
  now: number,
  locale: string,
): string {
  if (!iso) return EM_DASH;
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return EM_DASH;
  const diff = Math.round((t - now) / 1000);
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' });
  const abs = Math.abs(diff);
  if (abs < 60) return rtf.format(diff, 'second');
  if (abs < 3600) return rtf.format(Math.round(diff / 60), 'minute');
  if (abs < 86400) return rtf.format(Math.round(diff / 3600), 'hour');
  return rtf.format(Math.round(diff / 86400), 'day');
}

export function formatDuration(seconds: number, locale: string): string {
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const parts: string[] = [];
  const unitFmt = (value: number, unit: 'hour' | 'minute' | 'second') =>
    new Intl.NumberFormat(locale, { style: 'unit', unit, unitDisplay: 'narrow' }).format(value);
  if (h > 0) parts.push(unitFmt(h, 'hour'));
  if (m > 0 || h > 0) parts.push(unitFmt(m, 'minute'));
  if (h === 0 && m === 0) parts.push(unitFmt(s, 'second'));
  return parts.join(' ');
}
