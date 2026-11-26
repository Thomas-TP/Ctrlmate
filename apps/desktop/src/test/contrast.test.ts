import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { contrastRatio } from '../lib/contrast';

const css = readFileSync(resolve(process.cwd(), 'src/index.css'), 'utf8');

function tokens(selector: RegExp): Record<string, string> {
  const block = selector.exec(css)?.[1];
  if (!block) throw new Error(`token block not found: ${String(selector)}`);
  const out: Record<string, string> = {};
  for (const m of block.matchAll(/--([a-z0-9-]+):\s*(#[0-9a-fA-F]{6})\s*;/g)) {
    const [, name, value] = m;
    if (name && value) out[name] = value;
  }
  return out;
}

const light = tokens(/:root\s*\{([^}]*)\}/);
const dark = tokens(/:root\[data-theme='dark'\]\s*\{([^}]*)\}/);

/** [foreground, background, minimum ratio]. Text needs 4.5, graphics and focus rings 3. */
const PAIRS: [string, string, number][] = [
  ['fg', 'bg', 4.5],
  ['fg', 'surface', 4.5],
  ['fg', 'surface-2', 4.5],
  ['fg-muted', 'bg', 4.5],
  ['fg-muted', 'surface', 4.5],
  ['fg-muted', 'surface-2', 4.5],
  ['fg-muted', 'accent-soft', 4.5],
  ['fg-muted', 'warning-soft', 4.5],
  ['fg-muted', 'danger-soft', 4.5],
  ['fg-muted', 'success-soft', 4.5],
  ['fg-subtle', 'bg', 4.5],
  ['fg-subtle', 'surface', 4.5],
  ['fg-subtle', 'surface-2', 4.5],
  ['accent-fg', 'accent', 4.5],
  ['accent-fg', 'accent-hover', 4.5],
  ['accent-text', 'accent-soft', 4.5],
  ['accent-text', 'surface', 4.5],
  ['fg', 'accent-soft', 4.5],
  ['success', 'success-soft', 4.5],
  ['success', 'surface', 4.5],
  ['warning', 'warning-soft', 4.5],
  ['warning', 'surface', 4.5],
  ['danger', 'danger-soft', 4.5],
  ['danger', 'surface', 4.5],
  ['accent-fg', 'danger', 4.5],
  // Non-text: focus ring and chart strokes against their surface.
  ['focus', 'surface', 3],
  ['focus', 'bg', 3],
  ['chart-line', 'surface', 3],
  ['chart-ghost', 'surface', 3],
];

describe.each([
  ['light', light],
  ['dark', dark],
] as const)('%s theme tokens meet WCAG AA', (_name, t) => {
  it('defines every token used by the pairs', () => {
    for (const [fg, bg] of PAIRS) {
      expect(t[fg], fg).toBeDefined();
      expect(t[bg], bg).toBeDefined();
    }
  });

  it.each(PAIRS)('%s on %s >= %s', (fg, bg, min) => {
    const a = t[fg];
    const b = t[bg];
    if (!a || !b) throw new Error('missing token');
    expect(contrastRatio(a, b)).toBeGreaterThanOrEqual(min);
  });
});

describe('contrastRatio', () => {
  it('matches the WCAG reference values', () => {
    expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 5);
    expect(contrastRatio('#777777', '#ffffff')).toBeCloseTo(4.48, 1);
    expect(contrastRatio('#ffffff', '#ffffff')).toBeCloseTo(1, 5);
  });
});
