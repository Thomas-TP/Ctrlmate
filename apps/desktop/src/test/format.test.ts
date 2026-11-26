import { describe, expect, it } from 'vitest';
import {
  brightnessToPercent,
  hexToRgb,
  kelvinToRgb,
  percentToBrightness,
  rgbToHex,
} from '../lib/color';
import {
  EM_DASH,
  celsiusToUnit,
  formatCountdown,
  formatTemperature,
  formatTemperatureTick,
  groupFingerprint,
  unitToCelsius,
} from '../lib/format';

describe('formatTemperature', () => {
  it('formats Celsius with the unit symbol per locale', () => {
    expect(formatTemperature(47.4, 'celsius', 'en')).toBe('47°C');
    expect(formatTemperature(47.4, 'celsius', 'fr')).toMatch(/^47\s?°C$/);
  });

  it('converts to Fahrenheit in one place', () => {
    expect(formatTemperature(0, 'fahrenheit', 'en')).toBe('32°F');
    expect(formatTemperature(93, 'fahrenheit', 'en')).toBe('199°F');
    expect(celsiusToUnit(100, 'fahrenheit')).toBe(212);
    expect(unitToCelsius(212, 'fahrenheit')).toBeCloseTo(100);
  });

  it('treats deltas without the 32 degree offset', () => {
    expect(formatTemperature(5, 'fahrenheit', 'en', { delta: true })).toBe('9°F');
    expect(formatTemperature(5, 'celsius', 'en', { delta: true })).toBe('5°C');
  });

  it('never fabricates a value for a missing sensor', () => {
    expect(formatTemperature(null, 'celsius', 'en')).toBe(EM_DASH);
    expect(formatTemperature(undefined, 'fahrenheit', 'en')).toBe(EM_DASH);
    expect(formatTemperature(Number.NaN, 'celsius', 'en')).toBe(EM_DASH);
  });

  it('formats chart ticks as bare numbers in the display unit', () => {
    expect(formatTemperatureTick(50, 'celsius', 'en')).toBe('50');
    expect(formatTemperatureTick(50, 'fahrenheit', 'en')).toBe('122');
  });
});

describe('misc formatters', () => {
  it('formats the pairing countdown', () => {
    expect(formatCountdown(299_000)).toBe('04:59');
    expect(formatCountdown(0)).toBe('00:00');
    expect(formatCountdown(-5000)).toBe('00:00');
    expect(formatCountdown(61_000)).toBe('01:01');
  });

  it('groups a SHA-256 fingerprint in blocks of four', () => {
    const fp = 'a3f1c2d4e5b60718293a4b5c6d7e8f90112233445566778899aabbccddeeff00';
    const groups = groupFingerprint(fp);
    expect(groups).toHaveLength(16);
    expect(groups[0]).toBe('A3F1');
    expect(groups.join('')).toBe(fp.toUpperCase());
    expect(groupFingerprint('aa:bb:cc:dd')).toEqual(['AABB', 'CCDD']);
    expect(groupFingerprint('')).toEqual([]);
  });
});

describe('colour helpers', () => {
  it('round-trips hex and rgb', () => {
    expect(rgbToHex({ r: 255, g: 224, b: 189 })).toBe('#ffe0bd');
    expect(hexToRgb('#FFE0BD')).toEqual({ r: 255, g: 224, b: 189 });
    expect(hexToRgb('nope')).toBeNull();
  });

  it('maps the Home Assistant brightness scale', () => {
    expect(percentToBrightness(100)).toBe(255);
    expect(brightnessToPercent(204)).toBe(80);
    expect(brightnessToPercent(999)).toBe(100);
  });

  it('draws warm light redder than cool light', () => {
    const warm = kelvinToRgb(2000);
    const cool = kelvinToRgb(6500);
    expect(warm.b).toBeLessThan(cool.b);
    expect(warm.r).toBeGreaterThanOrEqual(cool.r - 1);
  });
});
