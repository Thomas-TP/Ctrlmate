import { useMemo } from 'react';
import { app } from '../api/app';
import { cmd, daemon } from '../api/daemon';
import { useAppInfo } from '../hooks/hooks';
import { useI18n } from '../i18n';
import { copyText } from '../lib/clipboard';
import { buildDiagnostics } from '../lib/diagnostics';
import { useActions } from '../store/actions';
import { useDaemon } from '../store/daemon';
import { SCREENS, useNav } from '../store/nav';
import { usePrefs } from '../store/prefs';
import { COOLING_PRESETS } from '../types/protocol';

export type CommandGroup = 'navigate' | 'cooling' | 'appearance' | 'actions' | 'app';

export interface PaletteCommand {
  id: string;
  group: CommandGroup;
  label: string;
  /** Extra words matched by the filter (not displayed). */
  keywords?: string;
  run: () => void | Promise<void>;
}

/** Same actions as the sidebar and the settings, reachable from Ctrl/Cmd+K. */
export function useCommands(): PaletteCommand[] {
  const { t, locale } = useI18n();
  const actions = useActions();
  const snapshot = useDaemon((s) => s.snapshot);
  const connection = useDaemon((s) => s.connection);
  const pushToast = useDaemon((s) => s.pushToast);
  const setScreen = useNav((s) => s.setScreen);
  const setLanguage = usePrefs((s) => s.setLanguage);
  const setTheme = usePrefs((s) => s.setTheme);
  const info = useAppInfo();

  return useMemo(() => {
    const list: PaletteCommand[] = [];
    for (const s of SCREENS) {
      list.push({
        id: `nav.${s}`,
        group: 'navigate',
        label: t('palette.go', { screen: t(`nav.${s}`) }),
        keywords: s,
        run: () => setScreen(s),
      });
    }
    if (snapshot) {
      for (const p of COOLING_PRESETS) {
        list.push({
          id: `cooling.${p}`,
          group: 'cooling',
          label: t('palette.cooling', { preset: t(`preset.${p}`) }),
          keywords: 'cooling fan refroidissement ventilation',
          run: async () => {
            await actions.run(cmd.setCoolingPreset(p), { successKey: 'palette.cooling.done' });
          },
        });
      }
    }
    for (const th of ['system', 'light', 'dark'] as const) {
      list.push({
        id: `theme.${th}`,
        group: 'appearance',
        label: t('palette.theme', {
          theme: t(
            th === 'system'
              ? 'settings.system'
              : th === 'light'
                ? 'settings.theme.light'
                : 'settings.theme.dark',
          ),
        }),
        keywords: 'theme thème dark light sombre clair',
        run: () => setTheme(th),
      });
    }
    for (const lang of ['system', 'fr', 'en'] as const) {
      list.push({
        id: `lang.${lang}`,
        group: 'appearance',
        label: t('palette.language', {
          language:
            lang === 'system' ? t('settings.system') : lang === 'fr' ? 'Français' : 'English',
        }),
        keywords: 'language langue',
        run: () => setLanguage(lang),
      });
    }
    if (snapshot) {
      for (const u of ['celsius', 'fahrenheit'] as const) {
        list.push({
          id: `unit.${u}`,
          group: 'appearance',
          label: t('palette.unit', { unit: u === 'celsius' ? '°C' : '°F' }),
          keywords: 'temperature unit fahrenheit celsius',
          run: async () => {
            await actions.settings({ temperature_unit: u });
          },
        });
      }
      list.push(
        {
          id: 'action.rescan',
          group: 'actions',
          label: t('palette.rescan'),
          keywords: 'hardware rgb scan',
          run: async () => {
            await actions.snapshotCall(() => daemon.rescanHardware(), {
              successKey: 'lighting.rescan.done',
            });
          },
        },
        {
          id: 'action.pairing',
          group: 'actions',
          label: t('palette.pairing'),
          keywords: 'home assistant pair pin',
          run: async () => {
            await actions.snapshotCall(() => daemon.startPairing());
            setScreen('homeAssistant');
          },
        },
      );
    }
    list.push(
      {
        id: 'action.reconnect',
        group: 'actions',
        label: t('palette.reconnect'),
        keywords: 'connection retry',
        run: async () => {
          await app.reconnect();
        },
      },
      {
        id: 'action.diagnostics',
        group: 'actions',
        label: t('palette.diagnostics'),
        keywords: 'support bundle copy',
        run: async () => {
          const ok = await copyText(
            buildDiagnostics({ app: info.data, connection, snapshot, locale }),
          );
          pushToast(
            ok ? 'success' : 'error',
            ok ? t('settings.diagnostics.copied') : t('common.copy_failed'),
          );
        },
      },
      {
        id: 'app.hide',
        group: 'app',
        label: t('palette.hide'),
        keywords: 'tray minimize',
        run: async () => {
          await app.hide();
        },
      },
      {
        id: 'app.quit',
        group: 'app',
        label: t('palette.quit'),
        keywords: 'exit close',
        run: async () => {
          await app.quit();
        },
      },
    );
    return list;
  }, [
    t,
    locale,
    actions,
    snapshot,
    connection,
    pushToast,
    setScreen,
    setLanguage,
    setTheme,
    info.data,
  ]);
}
