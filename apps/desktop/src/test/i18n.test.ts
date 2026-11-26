import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { catalogues, hasKey, resolveLocale, translate, translateCode } from '../i18n/core';
import { en } from '../i18n/en';
import { fr } from '../i18n/fr';
import { RESULT_CODES, SENSOR_REASONS, COOLING_REASONS } from '../types/protocol';
import { BRIDGE_ERROR_CODES } from '../api/tauri';

const REPO = resolve(process.cwd(), '..', '..');

function rust(rel: string): string {
  const p = join(REPO, rel);
  if (!existsSync(p)) throw new Error(`protocol source not found: ${rel}`);
  return readFileSync(p, 'utf8');
}

const snake = (s: string) => s.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase();

/** Variants of `pub enum <name> { ... }` (doc comments and attributes skipped). */
function enumVariants(src: string, name: string): string[] {
  const start = src.indexOf(`pub enum ${name} {`);
  if (start < 0) throw new Error(`enum ${name} not found`);
  const body = src.slice(start, src.indexOf('\n}', start));
  return [...body.matchAll(/^\s{4}([A-Z][A-Za-z0-9]*)\s*[,({]/gm)].map((m) => snake(m[1] ?? ''));
}

function structFields(src: string, name: string): string[] {
  const start = src.indexOf(`pub struct ${name} {`);
  if (start < 0) throw new Error(`struct ${name} not found`);
  const body = src.slice(start, src.indexOf('\n}', start));
  return [...body.matchAll(/^\s{4}pub (\w+):/gm)].map((m) => m[1] ?? '');
}

function walk(dir: string, out: string[] = []): string[] {
  for (const f of readdirSync(dir)) {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (p.endsWith('.rs')) out.push(p);
  }
  return out;
}

const params = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

describe('catalogue parity', () => {
  const enKeys = Object.keys(en).sort();
  const frKeys = Object.keys(fr).sort();

  it('has the same keys in French and English', () => {
    expect(
      frKeys.filter((k) => !enKeys.includes(k)),
      'only in fr',
    ).toEqual([]);
    expect(
      enKeys.filter((k) => !frKeys.includes(k)),
      'only in en',
    ).toEqual([]);
  });

  it('has no empty message', () => {
    for (const [k, v] of Object.entries(en)) expect(v.trim(), `en ${k}`).not.toBe('');
    for (const [k, v] of Object.entries(fr)) expect(v.trim(), `fr ${k}`).not.toBe('');
  });

  it('uses the same {placeholders} in both languages', () => {
    for (const k of enKeys) {
      const a = (en as Record<string, string>)[k] ?? '';
      const b = (fr as Record<string, string>)[k] ?? '';
      expect(params(b), k).toEqual(params(a));
    }
  });

  it('defines both plural forms together', () => {
    const bases = new Set(
      enKeys.filter((k) => /_(one|other)$/.test(k)).map((k) => k.replace(/_(one|other)$/, '')),
    );
    for (const b of bases) {
      expect(enKeys, `${b}_one`).toContain(`${b}_one`);
      expect(enKeys, `${b}_other`).toContain(`${b}_other`);
    }
  });
});

describe('every code the daemon or the Tauri side can send has a translation', () => {
  const both = (key: string) => {
    expect(hasKey('en', key), `en ${key}`).toBe(true);
    expect(hasKey('fr', key), `fr ${key}`).toBe(true);
  };

  it('result codes (read from the protocol crate)', () => {
    const src = rust('crates/ctrlmate-protocol/src/result.rs');
    const codes = [...src.matchAll(/Self::\w+ => "([a-z_]+)"/g)].map((m) => m[1] ?? '');
    expect(codes.length).toBeGreaterThanOrEqual(10);
    for (const c of codes) both(`result.${c}`);
    expect([...RESULT_CODES].sort()).toEqual([...codes].sort());
  });

  it('sensor reasons', () => {
    const variants = enumVariants(rust('crates/ctrlmate-protocol/src/status.rs'), 'SensorReason');
    expect(variants).toContain('requires_deep_hardware');
    for (const v of variants) both(`sensor.${v}`);
    expect([...SENSOR_REASONS].sort()).toEqual([...variants].sort());
  });

  it('cooling reasons, presets, polling profiles', () => {
    const cooling = enumVariants(rust('crates/ctrlmate-protocol/src/cooling.rs'), 'CoolingReason');
    for (const v of cooling) both(`cooling.reason.${v}`);
    expect([...COOLING_REASONS].sort()).toEqual([...cooling].sort());
    const lib = rust('crates/ctrlmate-protocol/src/lib.rs');
    for (const v of enumVariants(lib, 'CoolingPreset')) {
      both(`preset.${v}`);
      both(`preset.${v}.desc`);
    }
    for (const v of enumVariants(lib, 'PollingProfile')) {
      both(`polling.${v}`);
      both(`polling.${v}.desc`);
    }
    for (const v of enumVariants(lib, 'SecurityMode')) both(`security.${v}`);
  });

  it('permissions', () => {
    const lib = rust('crates/ctrlmate-protocol/src/lib.rs');
    const fields = structFields(lib, 'RemotePermissions');
    expect(fields.length).toBe(8);
    for (const f of fields) {
      both(`perm.${f}`);
      both(`perm.${f}.desc`);
    }
  });

  it('lighting reasons found in the rgb crate', () => {
    const files = walk(join(REPO, 'crates', 'ctrlmate-rgb', 'src'));
    const found = new Set<string>();
    for (const f of files) {
      for (const m of readFileSync(f, 'utf8').matchAll(/reason:\s*"([a-z0-9_]+)"/g))
        found.add(m[1] ?? '');
    }
    expect(found.size).toBeGreaterThan(0);
    for (const reason of found) {
      if (reason.startsWith('native_')) both('rgb.reason.native');
      else both(`rgb.reason.${reason}`);
    }
    // Reasons named by the architecture notes.
    for (const r of [
      'ok',
      'requires_deep_hardware',
      'not_present',
      'smbus_unavailable',
      'no_public_protocol',
      'no_accessories_detected',
      'via_openrgb',
    ]) {
      both(`rgb.reason.${r}`);
    }
  });

  it('Tauri bridge error codes (read from src-tauri/src/error.rs)', () => {
    const src = readFileSync(join(process.cwd(), 'src-tauri', 'src', 'error.rs'), 'utf8');
    const variants = enumVariants(src, 'ErrorCode');
    for (const v of variants) both(`connection.reason.${v}`);
    expect([...BRIDGE_ERROR_CODES].sort()).toEqual([...variants].sort());
  });

  it('every unknown code falls back to a readable sentence, never to a key', () => {
    for (const locale of ['en', 'fr'] as const) {
      for (const ns of [
        'result',
        'sensor',
        'rgb.reason',
        'cooling.reason',
        'connection.reason',
      ] as const) {
        const text = translateCode(locale, ns, 'brand_new_code');
        expect(text).toContain('brand_new_code');
        expect(text).not.toContain(`${ns}.`);
      }
    }
    expect(translateCode('en', 'rgb.reason', 'native_ene_smbus')).toContain('ene_smbus');
  });
});

describe('translate', () => {
  it('interpolates and picks plural forms with Intl.PluralRules', () => {
    expect(translate('en', 'system.updates.pending', { count: 1, formatted: '1' })).toBe(
      '1 update pending',
    );
    expect(translate('en', 'system.updates.pending', { count: 3, formatted: '3' })).toBe(
      '3 updates pending',
    );
    expect(translate('fr', 'system.updates.pending', { count: 0, formatted: '0' })).toBe(
      '0 mise à jour en attente',
    );
    expect(translate('fr', 'system.updates.pending', { count: 2, formatted: '2' })).toBe(
      '2 mises à jour en attente',
    );
  });

  it('resolves the system language', () => {
    expect(resolveLocale('system', ['fr-CH', 'en'])).toBe('fr');
    expect(resolveLocale('system', ['de-DE', 'en-GB'])).toBe('en');
    expect(resolveLocale('system', ['de'])).toBe('en');
    expect(resolveLocale('fr', ['en'])).toBe('fr');
  });

  it('exposes both catalogues', () => {
    expect(Object.keys(catalogues)).toEqual(['en', 'fr']);
  });
});
