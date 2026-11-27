import { create } from 'zustand';
import type { LanguagePref } from '../i18n/core';

export type ThemePref = 'system' | 'light' | 'dark';

const KEY = 'ctrlmate.prefs.v2';

interface Stored {
  language: LanguagePref;
  theme: ThemePref;
  /** The first-run onboarding was completed (or skipped) on this machine. */
  onboarded: boolean;
}

const DEFAULTS: Stored = { language: 'system', theme: 'system', onboarded: false };

function read(): Stored {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (!raw) return DEFAULTS;
    const v = JSON.parse(raw) as Partial<Stored>;
    return {
      language: v.language === 'fr' || v.language === 'en' ? v.language : 'system',
      theme: v.theme === 'light' || v.theme === 'dark' ? v.theme : 'system',
      onboarded: v.onboarded === true,
    };
  } catch {
    return DEFAULTS;
  }
}

function write(s: Stored): void {
  try {
    window.localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    /* storage unavailable: the preference lasts for this session only */
  }
}

interface PrefsState extends Stored {
  setLanguage: (l: LanguagePref) => void;
  setTheme: (t: ThemePref) => void;
  setOnboarded: (done: boolean) => void;
}

export const usePrefs = create<PrefsState>()((set, get) => {
  const persist = (patch: Partial<Stored>) => {
    set(patch);
    const { language, theme, onboarded } = get();
    write({ language, theme, onboarded });
  };
  return {
    ...read(),
    setLanguage: (language) => persist({ language }),
    setTheme: (theme) => persist({ theme }),
    setOnboarded: (onboarded) => persist({ onboarded }),
  };
});

// The tray panel is a second webview on the same origin: follow preference changes made in the
// main window (language, theme, onboarding).
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (e) => {
    if (e.key === KEY) usePrefs.setState(read());
  });
}
