import { useCallback, useEffect, useRef, useState } from 'react';
import {
  Activity,
  AlertTriangle,
  Cpu,
  Fan,
  Home,
  Sliders,
  Sun,
  Volume2,
  VolumeX,
} from 'lucide-react';
import {
  fetchSnapshot,
  formatTemp,
  ipcWsUrl,
  pressButton,
  rescanHardware,
  sendCommand,
  setCoolingPreset,
  setLight,
  updateSettings,
} from './api';
import { CommandPalette, type NavTab } from './components/CommandPalette';
import { TitleBar } from './components/TitleBar';
import { useSnapshotHelpers, useTelemetryHistory } from './hooks/useTelemetryHistory';
import type { CoolingPreset, DaemonSnapshot, TemperatureUnit } from './types';
import { CoolingView } from './views/CoolingView';
import { HardwareView } from './views/HardwareView';
import { HomeAssistantView } from './views/HomeAssistantView';
import { LightingView } from './views/LightingView';
import { OverviewView } from './views/OverviewView';
import { SystemView } from './views/SystemView';

const NAV_GROUPS: {
  title: string;
  items: { id: NavTab; label: string; shortcut: string; icon: typeof Activity }[];
}[] = [
  {
    title: 'Monitoring',
    items: [
      { id: 'overview', label: 'Tableau de bord', shortcut: '1', icon: Activity },
      { id: 'hardware', label: 'Matériel', shortcut: '2', icon: Cpu },
      { id: 'cooling', label: 'Refroidissement', shortcut: '3', icon: Fan },
      { id: 'lighting', label: 'Éclairage', shortcut: '4', icon: Sun },
    ],
  },
  {
    title: 'Contrôle',
    items: [
      { id: 'system', label: 'Système & Audio', shortcut: '5', icon: Sliders },
      { id: 'homeassistant', label: 'Home Assistant', shortcut: '6', icon: Home },
    ],
  },
];

const ALL_TABS: NavTab[] = [
  'overview',
  'hardware',
  'cooling',
  'lighting',
  'system',
  'homeassistant',
];

export default function App() {
  const [tab, setTab] = useState<NavTab>('overview');
  const [dark, setDark] = useState(true);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [snapshot, setSnapshot] = useState<DaemonSnapshot | null>(null);
  const [connected, setConnected] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const mainRef = useRef<HTMLElement>(null);

  useEffect(() => {
    mainRef.current?.scrollTo({ top: 0 });
  }, [tab]);

  const history = useTelemetryHistory(snapshot);
  const { numVal, boolVal, lightVal, updateVal } = useSnapshotHelpers(snapshot);

  const notify = useCallback((msg: string) => {
    setToast(msg);
    setTimeout(() => setToast((cur) => (cur === msg ? null : cur)), 3000);
  }, []);

  useEffect(() => {
    document.documentElement.classList.toggle('dark', dark);
  }, [dark]);

  // Global keyboard shortcuts: Ctrl+K Command Palette & 1-6 View Navigation
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setPaletteOpen((v) => !v);
        return;
      }
      if (paletteOpen) return;
      const target = e.target as HTMLElement | null;
      if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return;
      if (!e.ctrlKey && !e.metaKey && !e.altKey && e.key >= '1' && e.key <= '6') {
        const next = ALL_TABS[Number(e.key) - 1];
        if (next) setTab(next);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [paletteOpen]);

  // Live 1 Hz WebSocket stream from ctrlmated
  useEffect(() => {
    let ws: WebSocket | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let disposed = false;

    const connect = () => {
      if (disposed) return;
      ws = new WebSocket(ipcWsUrl());
      ws.onopen = () => setConnected(true);
      ws.onmessage = (ev) => {
        try {
          const data: DaemonSnapshot = JSON.parse(ev.data);
          setSnapshot(data);
        } catch {
          // ignore malformed frame
        }
      };
      ws.onclose = () => {
        setConnected(false);
        if (!disposed) {
          timer = setTimeout(connect, 2000);
        }
      };
      ws.onerror = () => ws?.close();
    };

    fetchSnapshot()
      .then((snap) => {
        if (!disposed) {
          setSnapshot(snap);
          setConnected(true);
        }
      })
      .catch(() => {});

    connect();

    return () => {
      disposed = true;
      if (timer) clearTimeout(timer);
      ws?.close();
    };
  }, []);

  const unit: TemperatureUnit = snapshot?.temperature_unit ?? 'celsius';
  const haConnectedCount = snapshot?.paired_clients.filter((c) => c.connected).length ?? 0;
  const liquidTemp = numVal('cooler_0_liquid_temperature');
  const pendingUpdates = updateVal()?.pending_count ?? 0;
  const lightOn = lightVal('light_global')?.is_on ?? false;
  const muted = boolVal('audio_mute');

  const toggleUnit = async () => {
    const nextUnit: TemperatureUnit = unit === 'celsius' ? 'fahrenheit' : 'celsius';
    const updated = await updateSettings({ temperature_unit: nextUnit });
    setSnapshot(updated);
  };

  const activeNavItem = NAV_GROUPS.flatMap((g) => g.items).find((i) => i.id === tab);

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-[var(--bg-canvas)] text-[var(--text-primary)]">
      {/* Custom Frameless Architectural TitleBar */}
      <TitleBar
        version={snapshot?.manifest.host.sw_version}
        hostName={snapshot?.manifest.host.name}
        securityMode={snapshot?.manifest.host.security_mode}
        unit={unit}
        dark={dark}
        onToggleUnit={toggleUnit}
        onToggleTheme={() => setDark((d) => !d)}
        onOpenPalette={() => setPaletteOpen(true)}
      />

      <div className="flex min-h-0 flex-1">
        {/* Refined 228px Linear-Style Navigation Rail */}
        <aside className="hairline-r flex w-[228px] shrink-0 flex-col justify-between bg-[var(--bg-chrome)] px-2.5 py-3.5">
          <nav className="space-y-5 overflow-y-auto">
            {NAV_GROUPS.map((group) => (
              <div key={group.title}>
                <div className="mb-1.5 px-2.5 text-[10px] font-medium tracking-[0.07em] text-[var(--text-muted)] uppercase">
                  {group.title}
                </div>
                <div className="space-y-0.5">
                  {group.items.map((item) => {
                    const Icon = item.icon;
                    const active = tab === item.id;
                    let badge: React.ReactNode = null;
                    if (item.id === 'cooling' && liquidTemp !== undefined) {
                      badge = (
                        <span className="tabular text-[10px] text-[var(--text-muted)]">
                          {formatTemp(liquidTemp, unit).replace(' ', '')}
                        </span>
                      );
                    } else if (item.id === 'system' && pendingUpdates > 0) {
                      badge = (
                        <span className="tabular rounded-full bg-blue-500/15 px-1.5 py-0.2 text-[10px] font-medium text-blue-500">
                          {pendingUpdates}
                        </span>
                      );
                    } else if (item.id === 'homeassistant' && haConnectedCount > 0) {
                      badge = <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />;
                    } else if (item.id === 'hardware' && (snapshot?.conflicts.length ?? 0) > 0) {
                      badge = <span className="h-1.5 w-1.5 rounded-full bg-amber-500" />;
                    }

                    return (
                      <button
                        key={item.id}
                        type="button"
                        onClick={() => setTab(item.id)}
                        className={`relative flex w-full items-center justify-between rounded-lg px-2.5 py-2 text-[12.5px] font-medium transition ${
                          active
                            ? 'bg-black/[0.06] text-[var(--text-primary)] dark:bg-white/[0.08] dark:text-white'
                            : 'text-[var(--text-secondary)] hover:bg-black/[0.035] hover:text-[var(--text-primary)] dark:hover:bg-white/[0.04]'
                        }`}
                      >
                        {active && (
                          <span className="absolute top-1.5 bottom-1.5 left-0 w-[2.5px] rounded-r-full bg-neutral-900 dark:bg-white" />
                        )}
                        <span className="flex items-center gap-2.5">
                          <Icon className="h-[15px] w-[15px] stroke-[1.75] opacity-85" />
                          <span>{item.label}</span>
                        </span>
                        {badge}
                      </button>
                    );
                  })}
                </div>
              </div>
            ))}
          </nav>

          {/* Bottom System Pulse & Quick Hardware Toggles */}
          <div className="hairline-t space-y-2.5 pt-3">
            <div className="space-y-1.5 rounded-xl bg-black/[0.025] p-2.5 text-[11px] ring-1 ring-[var(--border-hairline)] dark:bg-white/[0.025]">
              <div className="flex items-center justify-between">
                <span className="text-[var(--text-muted)]">Service ctrlmated</span>
                <span className="flex items-center gap-1.5 font-medium">
                  <span
                    className={`h-1.5 w-1.5 rounded-full ${
                      connected ? 'bg-emerald-500' : 'bg-amber-500'
                    }`}
                  />
                  {connected ? 'Actif' : 'Déconnecté'}
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-[var(--text-muted)]">Home Assistant</span>
                <span className="flex items-center gap-1.5 font-medium">
                  <span
                    className={`h-1.5 w-1.5 rounded-full ${
                      haConnectedCount > 0 ? 'bg-emerald-500' : 'bg-neutral-400'
                    }`}
                  />
                  {haConnectedCount > 0 ? 'Connecté' : 'Veille'}
                </span>
              </div>
            </div>

            {snapshot && (
              <div className="grid grid-cols-2 gap-1.5">
                <button
                  type="button"
                  onClick={() =>
                    setLight({ entity_id: 'light_global', is_on: !lightOn })
                  }
                  className={`flex items-center justify-center gap-1.5 rounded-lg py-1.5 text-[11px] font-medium ring-1 transition ${
                    lightOn
                      ? 'bg-neutral-900 text-white ring-neutral-900 dark:bg-white/[0.11] dark:text-white dark:ring-white/[0.1]'
                      : 'text-[var(--text-muted)] ring-[var(--border-hairline)] hover:text-[var(--text-primary)]'
                  }`}
                >
                  <Sun className="h-3 w-3" />
                  LEDs
                </button>
                <button
                  type="button"
                  onClick={() =>
                    sendCommand({
                      action: 'set_switch',
                      entity_id: 'audio_mute',
                      state: !muted,
                    })
                  }
                  className={`flex items-center justify-center gap-1.5 rounded-lg py-1.5 text-[11px] font-medium ring-1 transition ${
                    muted
                      ? 'bg-amber-500/10 text-amber-600 ring-amber-500/25 dark:text-amber-400'
                      : 'text-[var(--text-muted)] ring-[var(--border-hairline)] hover:text-[var(--text-primary)]'
                  }`}
                >
                  {muted ? <VolumeX className="h-3 w-3" /> : <Volume2 className="h-3 w-3" />}
                  {muted ? 'Muet' : 'Son'}
                </button>
              </div>
            )}
          </div>
        </aside>

        {/* Main Scrollable Content Canvas */}
        <main ref={mainRef} className="flex-1 overflow-y-auto px-7 py-6">
          {!snapshot ? (
            <div className="mx-auto mt-24 max-w-md">
              <div className="surface-card rounded-2xl p-7 text-center">
                <div className="mx-auto mb-3.5 flex h-11 w-11 items-center justify-center rounded-xl bg-[var(--bg-subtle)]">
                  <Activity className="h-5 w-5 animate-pulse text-[var(--text-muted)]" />
                </div>
                <h2 className="text-sm font-semibold">Connexion au daemon ctrlmated…</h2>
                <p className="mt-1.5 text-xs leading-relaxed text-[var(--text-muted)]">
                  Vérification du service local sur{' '}
                  <code className="tabular">127.0.0.1:9488</code>. Reconnexion automatique toutes
                  les 2 secondes.
                </p>
              </div>
            </div>
          ) : (
            <div className="mx-auto max-w-5xl space-y-5">
              {/* Crisp View Header */}
              <div className="flex items-end justify-between gap-4">
                <div>
                  <h1 className="text-[20px] leading-tight font-semibold tracking-[-0.025em]">
                    {activeNavItem?.label}
                  </h1>
                  <p className="mt-0.5 text-xs text-[var(--text-muted)]">
                    {snapshot.manifest.host.name} · {snapshot.manifest.host.os_version}
                  </p>
                </div>
                <span className="tabular hidden text-[11px] text-[var(--text-faint)] sm:inline">
                  Raccourcis : 1–6 · Ctrl+K
                </span>
              </div>

              {snapshot.thermal_failsafe_active && (
                <div className="flex items-center gap-3 rounded-xl bg-red-500/10 px-4 py-3 text-xs font-medium text-red-600 ring-1 ring-red-500/30 dark:text-red-300">
                  <AlertTriangle className="h-4 w-4 shrink-0" />
                  <span>
                    Sécurité thermique matérielle active : tous les canaux de refroidissement sont
                    forcés à 100 % jusqu’au retour sous les seuils.
                  </span>
                </div>
              )}

              {tab === 'overview' && (
                <OverviewView
                  snapshot={snapshot}
                  history={history}
                  unit={unit}
                  onNavigate={setTab}
                  onNotify={notify}
                />
              )}
              {tab === 'hardware' && (
                <HardwareView
                  snapshot={snapshot}
                  unit={unit}
                  onSnapshot={setSnapshot}
                  onNotify={notify}
                />
              )}
              {tab === 'cooling' && (
                <CoolingView snapshot={snapshot} unit={unit} onNotify={notify} />
              )}
              {tab === 'lighting' && (
                <LightingView
                  snapshot={snapshot}
                  onSnapshot={setSnapshot}
                  onNotify={notify}
                />
              )}
              {tab === 'system' && (
                <SystemView
                  snapshot={snapshot}
                  onSnapshot={setSnapshot}
                  onNotify={notify}
                />
              )}
              {tab === 'homeassistant' && (
                <HomeAssistantView
                  snapshot={snapshot}
                  onSnapshot={setSnapshot}
                  onNotify={notify}
                />
              )}
            </div>
          )}
        </main>
      </div>

      {/* Raycast / Linear-style Ctrl+K Command Palette */}
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        onNavigate={setTab}
        onToggleLights={() => setLight({ entity_id: 'light_global', is_on: !lightOn })}
        onKelvinPreset={(k) =>
          setLight({
            entity_id: 'light_global',
            is_on: true,
            color_temp_kelvin: k,
            transition_ms: 350,
          })
        }
        onCoolingPreset={async (preset: CoolingPreset) => {
          const r = await setCoolingPreset(preset);
          notify(r.message);
        }}
        onToggleMute={() =>
          sendCommand({ action: 'set_switch', entity_id: 'audio_mute', state: !muted })
        }
        onRescan={async () => {
          setSnapshot(await rescanHardware());
          notify('Matériel rescanné');
        }}
        onLock={async () => notify((await pressButton('power_lock')).message)}
        onDisplayOff={async () => notify((await pressButton('display_off')).message)}
        onToggleTheme={() => setDark((d) => !d)}
      />

      {/* Subtle Architectural Floating Toast */}
      {toast && (
        <div className="fixed right-5 bottom-5 z-40 rounded-xl bg-neutral-900 px-3.5 py-2 text-xs font-medium text-white shadow-xl ring-1 ring-white/10 dark:bg-white dark:text-neutral-950">
          {toast}
        </div>
      )}
    </div>
  );
}
