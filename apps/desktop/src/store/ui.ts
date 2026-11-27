import { create } from 'zustand';

export const SHEETS = [
  'cooling',
  'temps',
  'lighting',
  'ha',
  'system',
  'updates',
  'settings',
] as const;
export type SheetId = (typeof SHEETS)[number];

export const SETTINGS_TABS = ['general', 'hardware', 'advanced', 'about'] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];

interface UiState {
  sheet: SheetId | null;
  settingsTab: SettingsTab;
  paletteOpen: boolean;
  openSheet: (sheet: SheetId, tab?: SettingsTab) => void;
  closeSheet: () => void;
  setSettingsTab: (tab: SettingsTab) => void;
  setPaletteOpen: (open: boolean) => void;
}

export const useUi = create<UiState>()((set) => ({
  sheet: null,
  settingsTab: 'general',
  paletteOpen: false,
  openSheet: (sheet, tab) =>
    set((s) => ({ sheet, settingsTab: tab ?? (sheet === 'settings' ? s.settingsTab : 'general') })),
  closeSheet: () => set({ sheet: null }),
  setSettingsTab: (settingsTab) => set({ settingsTab }),
  setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
}));

export const isSheetId = (v: unknown): v is SheetId =>
  typeof v === 'string' && (SHEETS as readonly string[]).includes(v);
