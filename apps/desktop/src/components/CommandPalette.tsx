import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Activity,
  Cpu,
  Fan,
  Home,
  Lock,
  MonitorOff,
  Moon,
  RefreshCw,
  Search,
  Sliders,
  Sun,
  VolumeX,
} from 'lucide-react';
import type { CoolingPreset } from '../types';

export type NavTab = 'overview' | 'hardware' | 'cooling' | 'lighting' | 'system' | 'homeassistant';

export interface PaletteAction {
  id: string;
  group: string;
  label: string;
  hint?: string;
  icon: typeof Activity;
  run: () => void | Promise<void>;
}

export function CommandPalette({
  open,
  onClose,
  onNavigate,
  onToggleLights,
  onKelvinPreset,
  onCoolingPreset,
  onToggleMute,
  onRescan,
  onLock,
  onDisplayOff,
  onToggleTheme,
}: {
  open: boolean;
  onClose: () => void;
  onNavigate: (tab: NavTab) => void;
  onToggleLights: () => void;
  onKelvinPreset: (k: number) => void;
  onCoolingPreset: (preset: CoolingPreset) => void;
  onToggleMute: () => void;
  onRescan: () => void;
  onLock: () => void;
  onDisplayOff: () => void;
  onToggleTheme: () => void;
}) {
  const [query, setQuery] = useState('');
  const [selectedIdx, setSelectedIdx] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const actions = useMemo<PaletteAction[]>(
    () => [
      { id: 'nav-overview', group: 'Navigation', label: 'Tableau de bord', hint: '1', icon: Activity, run: () => onNavigate('overview') },
      { id: 'nav-hardware', group: 'Navigation', label: 'Matériel & capteurs', hint: '2', icon: Cpu, run: () => onNavigate('hardware') },
      { id: 'nav-cooling', group: 'Navigation', label: 'Refroidissement & courbes', hint: '3', icon: Fan, run: () => onNavigate('cooling') },
      { id: 'nav-lighting', group: 'Navigation', label: 'Éclairage RGB & Kelvin', hint: '4', icon: Sun, run: () => onNavigate('lighting') },
      { id: 'nav-system', group: 'Navigation', label: 'Système, Audio & Mises à jour', hint: '5', icon: Sliders, run: () => onNavigate('system') },
      { id: 'nav-ha', group: 'Navigation', label: 'Home Assistant & MQTT', hint: '6', icon: Home, run: () => onNavigate('homeassistant') },

      { id: 'light-toggle', group: 'Éclairage', label: 'Allumer / Éteindre les LEDs du PC', icon: Sun, run: onToggleLights },
      { id: 'light-2700', group: 'Éclairage', label: 'Ambiance Détente · 2700K (Blanc chaud)', icon: Sun, run: () => onKelvinPreset(2700) },
      { id: 'light-4000', group: 'Éclairage', label: 'Ambiance Neutre · 4000K', icon: Sun, run: () => onKelvinPreset(4000) },
      { id: 'light-6500', group: 'Éclairage', label: 'Ambiance Jour · 6500K', icon: Sun, run: () => onKelvinPreset(6500) },

      { id: 'cool-auto', group: 'Refroidissement', label: 'Mode Auto (courbe native)', icon: Fan, run: () => onCoolingPreset('auto') },
      { id: 'cool-silent', group: 'Refroidissement', label: 'Mode Silencieux', icon: Fan, run: () => onCoolingPreset('silent') },
      { id: 'cool-balanced', group: 'Refroidissement', label: 'Mode Équilibré', icon: Fan, run: () => onCoolingPreset('balanced') },
      { id: 'cool-perf', group: 'Refroidissement', label: 'Mode Performance', icon: Fan, run: () => onCoolingPreset('performance') },
      { id: 'cool-max', group: 'Refroidissement', label: 'Mode Maximum (100 %)', icon: Fan, run: () => onCoolingPreset('full_speed') },

      { id: 'sys-mute', group: 'Actions rapides', label: 'Couper / Rétablir le son', icon: VolumeX, run: onToggleMute },
      { id: 'sys-rescan', group: 'Actions rapides', label: 'Rescanner contrôleurs USB HID & RGB', icon: RefreshCw, run: onRescan },
      { id: 'sys-lock', group: 'Actions rapides', label: 'Verrouiller la session', icon: Lock, run: onLock },
      { id: 'sys-display', group: 'Actions rapides', label: 'Éteindre les écrans', icon: MonitorOff, run: onDisplayOff },
      { id: 'sys-theme', group: 'Actions rapides', label: 'Basculer thème Clair / Sombre', icon: Moon, run: onToggleTheme },
    ],
    [
      onNavigate,
      onToggleLights,
      onKelvinPreset,
      onCoolingPreset,
      onToggleMute,
      onRescan,
      onLock,
      onDisplayOff,
      onToggleTheme,
    ],
  );

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return actions;
    return actions.filter(
      (a) => a.label.toLowerCase().includes(q) || a.group.toLowerCase().includes(q),
    );
  }, [actions, query]);

  useEffect(() => {
    if (open) {
      setQuery('');
      setSelectedIdx(0);
      setTimeout(() => inputRef.current?.focus(), 10);
    }
  }, [open]);

  useEffect(() => {
    setSelectedIdx(0);
  }, [query]);

  if (!open) return null;

  const execute = (action: PaletteAction | undefined) => {
    if (!action) return;
    onClose();
    action.run();
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 pt-20 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Palette de commandes rapides"
        className="w-full max-w-lg overflow-hidden rounded-2xl bg-[var(--bg-elevated)] shadow-2xl ring-1 ring-black/15 dark:ring-white/[0.12]"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault();
            onClose();
          } else if (e.key === 'ArrowDown') {
            e.preventDefault();
            setSelectedIdx((i) => Math.min(filtered.length - 1, i + 1));
          } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            setSelectedIdx((i) => Math.max(0, i - 1));
          } else if (e.key === 'Enter') {
            e.preventDefault();
            execute(filtered[selectedIdx]);
          }
        }}
      >
        <div className="hairline-b flex items-center gap-2.5 px-4 py-3">
          <Search className="h-4 w-4 text-[var(--text-muted)]" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Rechercher une vue, un mode de refroidissement, une ambiance…"
            className="w-full bg-transparent text-sm text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:shadow-none focus:outline-none"
          />
          <kbd className="tabular rounded bg-black/[0.06] px-1.5 py-0.5 text-[10px] text-[var(--text-muted)] dark:bg-white/[0.08]">
            ESC
          </kbd>
        </div>

        <div className="max-h-80 overflow-y-auto p-1.5">
          {filtered.length === 0 ? (
            <div className="py-8 text-center text-xs text-[var(--text-muted)]">
              Aucune commande trouvée pour « {query} »
            </div>
          ) : (
            filtered.map((item, idx) => {
              const Icon = item.icon;
              const active = idx === selectedIdx;
              const prevGroup = idx > 0 ? filtered[idx - 1].group : null;
              const showGroup = item.group !== prevGroup;

              return (
                <div key={item.id}>
                  {showGroup && (
                    <div className="px-2.5 pt-2.5 pb-1 text-[10px] font-medium tracking-[0.06em] text-[var(--text-muted)] uppercase">
                      {item.group}
                    </div>
                  )}
                  <button
                    type="button"
                    onMouseEnter={() => setSelectedIdx(idx)}
                    onClick={() => execute(item)}
                    className={`flex w-full items-center justify-between rounded-lg px-2.5 py-2 text-left text-[13px] transition ${
                      active
                        ? 'bg-neutral-900 text-white dark:bg-white/[0.1] dark:text-white'
                        : 'text-[var(--text-secondary)] hover:bg-black/[0.04] dark:hover:bg-white/[0.04]'
                    }`}
                  >
                    <span className="flex items-center gap-2.5">
                      <Icon className="h-3.5 w-3.5 shrink-0 opacity-80" />
                      <span>{item.label}</span>
                    </span>
                    {item.hint && (
                      <kbd
                        className={`tabular rounded px-1.5 py-0.5 text-[10px] ${
                          active
                            ? 'bg-white/15 text-white'
                            : 'bg-black/[0.05] text-[var(--text-muted)] dark:bg-white/[0.06]'
                        }`}
                      >
                        {item.hint}
                      </kbd>
                    )}
                  </button>
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
