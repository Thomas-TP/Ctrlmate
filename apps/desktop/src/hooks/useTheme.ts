import { useEffect } from 'react';
import { app } from '../api/app';
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

/**
 * Applies the theme preference (following the system). The main window also aligns the native
 * title bar; the tray panel has no frame.
 */
export function useTheme(nativeFrame: boolean): void {
  const pref = usePrefs((s) => s.theme);
  useEffect(() => {
    const apply = () => {
      document.documentElement.dataset.theme = resolveTheme(pref, systemPrefersDark());
    };
    apply();
    if (nativeFrame) {
      app.setWindowTheme(pref === 'system' ? null : pref).catch(() => {
        /* not running inside Tauri */
      });
    }
    if (pref !== 'system' || typeof window.matchMedia !== 'function') return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', apply);
    return () => mq.removeEventListener('change', apply);
  }, [pref, nativeFrame]);
}
