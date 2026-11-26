import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { useDaemon } from '../store/daemon';
import { usePrefs } from '../store/prefs';
import {
  formatNumber,
  formatPercent,
  formatTemperature,
  formatDateTime,
  formatRelative,
  type TempFormatOptions,
} from '../lib/format';
import type { TemperatureUnit } from '../types/protocol';
import {
  resolveLocale,
  translate,
  translateCode,
  type CodeNamespace,
  type Locale,
  type Params,
  type TKey,
} from './core';

export interface I18n {
  locale: Locale;
  t: (key: TKey, params?: Params) => string;
  /** Translate a daemon/Tauri code in a namespace (`result`, `sensor`, `rgb.reason`...). */
  code: (ns: CodeNamespace, code: string) => string;
  unit: TemperatureUnit;
  temp: (celsius: number | null | undefined, opts?: TempFormatOptions) => string;
  num: (v: number | null | undefined, digits?: number) => string;
  pct: (v: number | null | undefined, digits?: number) => string;
  date: (iso: string | null | undefined) => string;
  ago: (iso: string | null | undefined) => string;
}

const Ctx = createContext<I18n | null>(null);

function systemLanguages(): readonly string[] {
  return typeof navigator === 'undefined'
    ? []
    : navigator.languages.length
      ? navigator.languages
      : [navigator.language];
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const pref = usePrefs((s) => s.language);
  const unit = useDaemon((s) => s.snapshot?.temperature_unit ?? 'celsius');
  const [sys, setSys] = useState(systemLanguages);
  useEffect(() => {
    const on = () => setSys(systemLanguages());
    window.addEventListener('languagechange', on);
    return () => window.removeEventListener('languagechange', on);
  }, []);
  const locale = resolveLocale(pref, sys);

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  const t = useCallback((key: TKey, params?: Params) => translate(locale, key, params), [locale]);
  const value = useMemo<I18n>(
    () => ({
      locale,
      t,
      code: (ns, code) => translateCode(locale, ns, code),
      unit,
      temp: (c, opts) => formatTemperature(c, unit, locale, opts),
      num: (v, d) => formatNumber(v, locale, d),
      pct: (v, d) => formatPercent(v, locale, d),
      date: (iso) => formatDateTime(iso, locale),
      ago: (iso) => formatRelative(iso, Date.now(), locale),
    }),
    [locale, t, unit],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useI18n(): I18n {
  const v = useContext(Ctx);
  if (!v) throw new Error('useI18n must be used inside <I18nProvider>');
  return v;
}
