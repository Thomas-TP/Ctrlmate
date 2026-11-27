import { useEffect, useMemo, useState } from 'react';
import { Search } from 'lucide-react';
import { app } from '../../api/app';
import { cmd, daemon } from '../../api/daemon';
import { useI18n } from '../../i18n';
import { copyText } from '../../lib/clipboard';
import { buildDiagnostics } from '../../lib/diagnostics';
import { lightZones } from '../../lib/snapshot';
import { useActions } from '../../store/actions';
import { useDaemon } from '../../store/daemon';
import { usePrefs } from '../../store/prefs';
import { SHEETS, useUi, type SheetId } from '../../store/ui';
import { COOLING_PRESETS, type DaemonSnapshot } from '../../types/protocol';
import { cn, Badge } from '../../ui/kit';
import { PaletteDialog } from '../../ui/overlay';

interface Command {
  id: string;
  group: string;
  label: string;
  run: () => unknown;
}

const SHEET_LABEL: Record<
  SheetId,
  | 'nav.cooling'
  | 'dash.temps.title'
  | 'nav.lighting'
  | 'nav.homeAssistant'
  | 'dash.pc.title'
  | 'dash.updates.title'
  | 'nav.settings'
> = {
  cooling: 'nav.cooling',
  temps: 'dash.temps.title',
  lighting: 'nav.lighting',
  ha: 'nav.homeAssistant',
  system: 'dash.pc.title',
  updates: 'dash.updates.title',
  settings: 'nav.settings',
};

function useCommands(snap: DaemonSnapshot | null): Command[] {
  const { t, locale } = useI18n();
  const actions = useActions();
  const openSheet = useUi((s) => s.openSheet);
  const connection = useDaemon((s) => s.connection);
  const pushToast = useDaemon((s) => s.pushToast);
  return useMemo(() => {
    const list: Command[] = SHEETS.map((id) => ({
      id: `go:${id}`,
      group: t('palette.group.navigate'),
      label: t('palette.go', { screen: t(SHEET_LABEL[id]) }),
      run: () => openSheet(id),
    }));
    const { setTheme, setLanguage } = usePrefs.getState();
    if (snap) {
      for (const p of COOLING_PRESETS) {
        list.push({
          id: `preset:${p}`,
          group: t('palette.group.cooling'),
          label: t('palette.cooling', { preset: t(`preset.${p}`) }),
          run: () => actions.run(cmd.setCoolingPreset(p), { successKey: 'palette.cooling.done' }),
        });
      }
      const master = lightZones(snap).find((z) => z.isMaster);
      if (master?.state) {
        const on = master.state.is_on;
        list.push({
          id: 'light:toggle',
          group: t('nav.lighting'),
          label: on ? t('palette.light.off') : t('palette.light.on'),
          run: () => actions.run(cmd.light(master.id, { is_on: !on })),
        });
        for (const [key, k] of [
          ['light.warm', 2700],
          ['light.neutral', 4000],
          ['light.cool', 6000],
        ] as const) {
          list.push({
            id: `light:${k}`,
            group: t('nav.lighting'),
            label: `${t('nav.lighting')}: ${t(key)}`,
            run: () => actions.run(cmd.light(master.id, { is_on: true, color_temp_kelvin: k })),
          });
        }
      }
      list.push(
        {
          id: 'act:lock',
          group: t('palette.group.actions'),
          label: t('power.power_lock'),
          run: () => actions.run(cmd.press('power_lock')),
        },
        {
          id: 'act:display',
          group: t('palette.group.actions'),
          label: t('power.display_off'),
          run: () => actions.run(cmd.press('display_off')),
        },
        {
          id: 'act:rescan',
          group: t('palette.group.actions'),
          label: t('palette.rescan'),
          run: () =>
            actions.snapshotCall(() => daemon.rescanHardware(), {
              successKey: 'lighting.rescan.done',
            }),
        },
        {
          id: 'act:pairing',
          group: t('palette.group.actions'),
          label: t('palette.pairing'),
          run: async () => {
            await actions.snapshotCall(() => daemon.startPairing());
            openSheet('ha');
          },
        },
        {
          id: 'act:unit',
          group: t('palette.group.appearance'),
          label: t('palette.unit', { unit: snap.temperature_unit === 'celsius' ? '°F' : '°C' }),
          run: () =>
            actions.settings({
              temperature_unit: snap.temperature_unit === 'celsius' ? 'fahrenheit' : 'celsius',
            }),
        },
      );
    }
    list.push(
      {
        id: 'theme:light',
        group: t('palette.group.appearance'),
        label: t('palette.theme', { theme: t('settings.theme.light') }),
        run: () => setTheme('light'),
      },
      {
        id: 'theme:dark',
        group: t('palette.group.appearance'),
        label: t('palette.theme', { theme: t('settings.theme.dark') }),
        run: () => setTheme('dark'),
      },
      {
        id: 'theme:system',
        group: t('palette.group.appearance'),
        label: t('palette.theme', { theme: t('settings.system') }),
        run: () => setTheme('system'),
      },
      {
        id: 'lang:fr',
        group: t('palette.group.appearance'),
        label: t('palette.language', { language: 'Français' }),
        run: () => setLanguage('fr'),
      },
      {
        id: 'lang:en',
        group: t('palette.group.appearance'),
        label: t('palette.language', { language: 'English' }),
        run: () => setLanguage('en'),
      },
      {
        id: 'app:reconnect',
        group: t('palette.group.app'),
        label: t('palette.reconnect'),
        run: () => app.reconnect(),
      },
      {
        id: 'app:diag',
        group: t('palette.group.app'),
        label: t('palette.diagnostics'),
        run: async () => {
          const info = await app.info().catch(() => null);
          const ok = await copyText(
            buildDiagnostics({ app: info, connection, snapshot: snap, locale }),
          );
          pushToast(
            ok ? 'success' : 'error',
            ok ? t('settings.diagnostics.copied') : t('common.copy_failed'),
          );
        },
      },
      {
        id: 'app:hide',
        group: t('palette.group.app'),
        label: t('palette.hide'),
        run: () => app.hide(),
      },
      {
        id: 'app:quit',
        group: t('palette.group.app'),
        label: t('palette.quit'),
        run: () => app.quit(),
      },
    );
    return list;
  }, [t, locale, snap, actions, openSheet, connection, pushToast]);
}

const norm = (s: string) => s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

export function Palette({ snap }: { snap: DaemonSnapshot | null }) {
  const { t } = useI18n();
  const open = useUi((s) => s.paletteOpen);
  const setOpen = useUi((s) => s.setPaletteOpen);
  const commands = useCommands(snap);
  const [query, setQuery] = useState('');
  const [index, setIndex] = useState(0);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        useUi.getState().setPaletteOpen(!useUi.getState().paletteOpen);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const results = useMemo(() => {
    const words = norm(query).split(/\s+/).filter(Boolean);
    return commands.filter((c) => {
      const hay = norm(`${c.group} ${c.label}`);
      return words.every((w) => hay.includes(w));
    });
  }, [commands, query]);

  const close = () => {
    setOpen(false);
    setQuery('');
    setIndex(0);
  };
  const run = (c: Command | undefined) => {
    if (!c) return;
    close();
    void Promise.resolve(c.run());
  };
  const active = Math.min(index, Math.max(results.length - 1, 0));

  return (
    <PaletteDialog open={open} onClose={close} label={t('palette.title')}>
      <div className="flex items-center gap-3 border-b border-line px-4">
        <Search className="size-4 shrink-0 text-subtle" aria-hidden />
        <input
          autoFocus
          role="combobox"
          aria-expanded
          aria-controls="palette-list"
          aria-activedescendant={results[active] ? `pal-${results[active].id}` : undefined}
          value={query}
          placeholder={t('palette.placeholder')}
          onChange={(e) => {
            setQuery(e.target.value);
            setIndex(0);
          }}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown') {
              e.preventDefault();
              setIndex(Math.min(active + 1, results.length - 1));
            } else if (e.key === 'ArrowUp') {
              e.preventDefault();
              setIndex(Math.max(active - 1, 0));
            } else if (e.key === 'Enter') {
              e.preventDefault();
              run(results[active]);
            }
          }}
          style={{ outline: 'none' }}
          className="h-14 flex-1 bg-transparent text-[15px] text-fg placeholder:text-subtle"
        />
        <Badge>Esc</Badge>
      </div>
      <ul
        id="palette-list"
        role="listbox"
        aria-label={t('palette.title')}
        className="max-h-[50vh] overflow-y-auto p-2"
      >
        {results.length === 0 ? (
          <li className="px-3 py-6 text-center text-sm text-muted">{t('palette.empty')}</li>
        ) : (
          results.map((c, i) => (
            <li
              key={c.id}
              id={`pal-${c.id}`}
              role="option"
              aria-selected={i === active}
              onMouseMove={() => setIndex(i)}
              onClick={() => run(c)}
              className={cn(
                'flex cursor-pointer items-center justify-between gap-4 rounded-lg px-3 py-2.5 text-sm',
                i === active ? 'bg-accent-soft text-fg' : 'text-fg',
              )}
            >
              <span>{c.label}</span>
              <span className="text-xs text-subtle">{c.group}</span>
            </li>
          ))
        )}
      </ul>
    </PaletteDialog>
  );
}
