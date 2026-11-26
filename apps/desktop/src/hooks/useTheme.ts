import { useEffect } from 'react';
import { app } from '../api/app';
import { useI18n } from '../i18n';
import { usePrefs, type ThemePref } from '../store/prefs';

export type ResolvedTheme = 'light' | 'dark';

export function resolveTheme(pref: ThemePref, systemDark: boolean): ResolvedTheme {
  return pref === 'system' ? (systemDark ? 'dark' : 'light') : pref;
}

const systemPrefersDark = (): boolean =>
  typeof window !== 'undefined' &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-color-scheme: dark)').matches;

/** Sets `data-theme` before React renders to avoid a flash. */
export function applyThemeEarly(pref: ThemePref): void {
  document.documentElement.dataset.theme = resolveTheme(pref, systemPrefersDark());
}

/** Applies the theme preference (following the system) and aligns the native window frame. */
export function useTheme(): void {
  const pref = usePrefs((s) => s.theme);
  useEffect(() => {
    const apply = () => {
      document.documentElement.dataset.theme = resolveTheme(pref, systemPrefersDark());
    };
    apply();
    app.setWindowTheme(pref === 'system' ? null : pref).catch(() => {
      /* not running inside Tauri */
    });
    if (pref !== 'system' || typeof window.matchMedia !== 'function') return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', apply);
    return () => mq.removeEventListener('change', apply);
  }, [pref]);
}

/** Pushes the translated tray labels to the Rust side whenever the language changes. */
export function useTrayLabels(): void {
  const { t, locale } = useI18n();
  useEffect(() => {
    app.setTrayLabels(t('tray.show'), t('tray.quit')).catch(() => {
      /* not running inside Tauri */
    });
  }, [t, locale]);
}
