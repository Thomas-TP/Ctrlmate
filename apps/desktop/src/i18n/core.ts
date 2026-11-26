import { en } from './en';
import { fr } from './fr';

export type Locale = 'en' | 'fr';
export type LanguagePref = 'system' | Locale;
export const LOCALES: readonly Locale[] = ['en', 'fr'];

export type MessageKey = keyof typeof en;
/** Plural variants (`key_one` / `key_other`) are addressed by their base key. */
type PluralBase<K extends string> = K extends `${infer B}_one`
  ? B
  : K extends `${infer B}_other`
    ? B
    : K;
export type TKey = PluralBase<MessageKey>;
export type Params = Readonly<Record<string, string | number>>;
export type Translate = (key: TKey, params?: Params) => string;

export const catalogues: Readonly<Record<Locale, Readonly<Record<string, string>>>> = { en, fr };

export function resolveLocale(pref: LanguagePref, systemLanguages: readonly string[]): Locale {
  if (pref !== 'system') return pref;
  for (const l of systemLanguages) {
    const base = l.toLowerCase().split('-')[0];
    if (base === 'fr') return 'fr';
    if (base === 'en') return 'en';
  }
  return 'en';
}

function interpolate(template: string, params?: Params): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (m, name: string) => {
    const v = params[name];
    return v === undefined ? m : String(v);
  });
}

export function hasKey(locale: Locale, key: string): boolean {
  return key in catalogues[locale];
}

/** Looks up `key`, picking the `_one` / `_other` plural form when `params.count` is given. */
export function translate(locale: Locale, key: string, params?: Params): string {
  const cat = catalogues[locale];
  let template: string | undefined;
  const count = params?.count;
  if (typeof count === 'number') {
    const rule = new Intl.PluralRules(locale).select(count);
    template = cat[`${key}_${rule}`] ?? cat[`${key}_other`];
  }
  template ??= cat[key];
  // Never leak an empty string: a missing key renders the key itself (tests forbid this).
  return interpolate(template ?? key, params);
}

export type CodeNamespace =
  'result' | 'sensor' | 'rgb.reason' | 'cooling.reason' | 'connection.reason';

/**
 * Translates a daemon / Tauri code. Unknown codes from a newer peer fall back to
 * `<namespace>.unknown` (with the raw code as a parameter), never to an empty string.
 */
export function translateCode(locale: Locale, ns: CodeNamespace, code: string): string {
  const direct = `${ns}.${code}`;
  if (hasKey(locale, direct)) return translate(locale, direct);
  if (ns === 'rgb.reason' && code.startsWith('native_')) {
    return translate(locale, 'rgb.reason.native', { backend: code.slice('native_'.length) });
  }
  return translate(locale, `${ns}.unknown`, { code });
}
