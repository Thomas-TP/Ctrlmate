import { create } from 'zustand';

export const SCREENS = [
  'overview',
  'cooling',
  'lighting',
  'system',
  'homeAssistant',
  'settings',
] as const;
export type ScreenId = (typeof SCREENS)[number];

interface NavState {
  screen: ScreenId;
  setScreen: (s: ScreenId) => void;
  paletteOpen: boolean;
  setPaletteOpen: (open: boolean) => void;
}

export const useNav = create<NavState>()((set) => ({
  screen: 'overview',
  setScreen: (screen) => set({ screen }),
  paletteOpen: false,
  setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
}));
