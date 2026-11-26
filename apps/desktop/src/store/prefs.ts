import { create } from 'zustand';
import type { LanguagePref } from '../i18n/core';

export type ThemePref = 'system' | 'light' | 'dark';

const KEY = 'ctrlmate.prefs.v1';

interface Stored {
  language: LanguagePref;
  theme: ThemePref;
}

const DEFAULTS: Stored = { language: 'system', theme: 'system' };

function read(): Stored {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (!raw) return DEFAULTS;
    const v = JSON.parse(raw) as Partial<Stored>;
    return {
      language: v.language === 'fr' || v.language === 'en' ? v.language : 'system',
      theme: v.theme === 'light' || v.theme === 'dark' ? v.theme : 'system',
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
}

export const usePrefs = create<PrefsState>()((set, get) => ({
  ...read(),
  setLanguage: (language) => {
    set({ language });
    write({ language, theme: get().theme });
  },
  setTheme: (theme) => {
    set({ theme });
    write({ language: get().language, theme });
  },
}));
