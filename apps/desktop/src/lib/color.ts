import type { RgbColor } from '../types/protocol';

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

export function rgbToHex({ r, g, b }: RgbColor): string {
  const h = (n: number) => clamp(Math.round(n), 0, 255).toString(16).padStart(2, '0');
  return `#${h(r)}${h(g)}${h(b)}`;
}

export function hexToRgb(hex: string): RgbColor | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  const digits = m?.[1];
  if (!digits) return null;
  const n = parseInt(digits, 16);
  return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 };
}

/** Home Assistant brightness scale (0-255) to percent. */
export function brightnessToPercent(b: number): number {
  return Math.round((clamp(b, 0, 255) / 255) * 100);
}

export function percentToBrightness(p: number): number {
  return Math.round((clamp(p, 0, 100) / 100) * 255);
}

/** Approximate sRGB colour of a black body at `kelvin` (Tanner Helland), for the slider track. */
export function kelvinToRgb(kelvin: number): RgbColor {
  const t = clamp(kelvin, 1000, 40000) / 100;
  const r = t <= 66 ? 255 : 329.698727446 * Math.pow(t - 60, -0.1332047592);
  const g =
    t <= 66
      ? 99.4708025861 * Math.log(t) - 161.1195681661
      : 288.1221695283 * Math.pow(t - 60, -0.0755148492);
  const b = t >= 66 ? 255 : t <= 19 ? 0 : 138.5177312231 * Math.log(t - 10) - 305.0447927307;
  return { r: clamp(r, 0, 255), g: clamp(g, 0, 255), b: clamp(b, 0, 255) };
}

export function kelvinGradient(min: number, max: number, stops = 8): string {
  const parts: string[] = [];
  for (let i = 0; i < stops; i++) {
    const k = min + ((max - min) * i) / (stops - 1);
    parts.push(`${rgbToHex(kelvinToRgb(k))} ${Math.round((i / (stops - 1)) * 100)}%`);
  }
  return `linear-gradient(90deg, ${parts.join(', ')})`;
}
