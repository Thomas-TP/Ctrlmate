import { useEffect, useMemo, useState } from 'react';
import {
  Activity,
  AlertTriangle,
  AppWindow,
  Bell,
  Cpu,
  Download,
  Fan,
  Gamepad2,
  HardDrive,
  Home,
  KeyRound,
  Lock,
  Mic,
  MonitorOff,
  Moon,
  Play,
  Plus,
  Power,
  RefreshCw,
  ShieldCheck,
  Sliders,
  Sun,
  Trash2,
  Video,
  Volume2,
  VolumeX,
} from 'lucide-react';
import {
  cancelPairing,
  fetchSnapshot,
  formatTemp,
  ipcWsUrl,
  kelvinToPreviewHex,
  pressButton,
  rescanHardware,
  revokeClient,
  sendCommand,
  setCoolingPreset,
  setLight,
  startPairing,
  updateSettings,
} from './api';
import type {
  AllowlistedScript,
  CoolingPreset,
  DaemonSnapshot,
  EntityState,
  LightState,
  OsUpdateState,
  PollingProfile,
  RemotePermissions,
  RgbColor,
  TemperatureUnit,
} from './types';

type NavTab = 'overview' | 'hardware' | 'lighting' | 'cooling' | 'system' | 'homeassistant';

const PRESET_LABELS: Record<CoolingPreset, { label: string; desc: string }> = {
  auto: { label: 'Auto', desc: 'Courbe matérielle / BIOS native' },
  silent: { label: 'Silencieux', desc: 'Ventilation réduite, pompe protégée' },
  balanced: { label: 'Équilibré', desc: 'Compromis acoustique et thermique' },
  performance: { label: 'Performance', desc: 'Flux d’air soutenu pour forte charge' },
  full_speed: { label: 'Maximum', desc: 'Refroidissement 100 % immédiat' },
};

const COLOR_SWATCHES: { name: string; rgb: RgbColor }[] = [
  { name: 'Blanc chaud', rgb: { r: 255, g: 196, b: 138 } },
  { name: 'Blanc neutre', rgb: { r: 255, g: 244, b: 234 } },
  { name: 'Ambre doux', rgb: { r: 245, g: 158, b: 82 } },
  { name: 'Corail', rgb: { r: 235, g: 108, b: 92 } },
  { name: 'Sauge', rgb: { r: 120, g: 186, b: 146 } },
  { name: 'Ardoise', rgb: { r: 112, g: 156, b: 214 } },
  { name: 'Lavande', rgb: { r: 162, g: 144, b: 212 } },
];

export default function App() {
  const [tab, setTab] = useState<NavTab>('overview');
  const [dark, setDark] = useState(true);
  const [snapshot, setSnapshot] = useState<DaemonSnapshot | null>(null);
  const [connected, setConnected] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [selectedZone, setSelectedZone] = useState<string>('light_global');
  const [newScriptName, setNewScriptName] = useState('');
  const [newScriptExe, setNewScriptExe] = useState('');
  const [newScriptArgs, setNewScriptArgs] = useState('');

  useEffect(() => {
    document.documentElement.classList.toggle('dark', dark);
  }, [dark]);

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

  const notify = (msg: string) => {
    setToast(msg);
    setTimeout(() => setToast((cur) => (cur === msg ? null : cur)), 3200);
  };

  const stateMap = useMemo(() => {
    const map = new Map<string, EntityState>();
    if (snapshot) {
      for (const s of snapshot.states) {
        map.set(s.id, s);
      }
    }
    return map;
  }, [snapshot]);

  const numVal = (id: string): number | undefined => {
    const s = stateMap.get(id);
    if (!s) return undefined;
    if (s.value.kind === 'float' || s.value.kind === 'int') return s.value.value;
    return undefined;
  };

  const boolVal = (id: string): boolean => {
    const s = stateMap.get(id);
    return s?.value.kind === 'bool' ? s.value.value : false;
  };

  const textVal = (id: string): string => {
    const s = stateMap.get(id);
    return s?.value.kind === 'text' ? s.value.value : '';
  };

  const lightVal = (id: string): LightState | undefined => {
    const s = stateMap.get(id);
    return s?.value.kind === 'light' ? s.value.value : undefined;
  };

  const updateVal = (): OsUpdateState | undefined => {
    const s = stateMap.get('os_updates');
    return s?.value.kind === 'update' ? s.value.value : undefined;
  };

  const audioOutputs = useMemo(() => {
    const desc = snapshot?.manifest.entities.find((e) => e.id === 'audio_output');
    return desc?.options ?? [];
  }, [snapshot]);

  const unit: TemperatureUnit = snapshot?.temperature_unit ?? 'celsius';
  const haConnectedCount = snapshot?.paired_clients.filter((c) => c.connected).length ?? 0;

  const navItems: { id: NavTab; label: string; icon: typeof Activity }[] = [
    { id: 'overview', label: 'Vue globale', icon: Activity },
    { id: 'hardware', label: 'Matériel', icon: Cpu },
    { id: 'lighting', label: 'Éclairage', icon: Sun },
    { id: 'cooling', label: 'Refroidissement', icon: Fan },
    { id: 'system', label: 'Système', icon: Sliders },
    { id: 'homeassistant', label: 'Home Assistant', icon: Home },
  ];

  const lightZones = useMemo(() => {
    if (!snapshot) return [];
    return snapshot.manifest.entities.filter((e) => e.platform === 'light');
  }, [snapshot]);

  const activeLight = lightVal(selectedZone) ?? lightVal('light_global');

  return (
    <div className="flex h-screen overflow-hidden bg-neutral-50 text-neutral-900 dark:bg-[#0b0c0e] dark:text-[#eef0f3]">
      {/* Minimalist Sidebar */}
      <aside className="flex w-60 shrink-0 flex-col border-r border-neutral-200/80 bg-white/70 px-3 py-4 dark:border-white/[0.07] dark:bg-[#0f1115]">
        <div className="mb-6 flex items-center justify-between px-2.5">
          <div className="flex items-center gap-2.5">
            <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-neutral-900 text-xs font-semibold text-white dark:bg-white dark:text-neutral-900">
              C
            </div>
            <div>
              <div className="text-sm font-semibold tracking-tight">Ctrlmate</div>
              <div className="text-[11px] text-neutral-500">
                {snapshot ? `v${snapshot.manifest.host.sw_version}` : 'Hors ligne'}
              </div>
            </div>
          </div>
          <button
            onClick={() => setDark((d) => !d)}
            title="Changer de thème"
            className="rounded-md p-1.5 text-neutral-500 transition hover:bg-neutral-100 hover:text-neutral-900 dark:hover:bg-white/[0.06] dark:hover:text-white"
          >
            {dark ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
          </button>
        </div>

        <nav className="flex-1 space-y-0.5">
          {navItems.map((item) => {
            const Icon = item.icon;
            const active = tab === item.id;
            return (
              <button
                key={item.id}
                onClick={() => setTab(item.id)}
                className={`flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-[13px] font-medium transition ${
                  active
                    ? 'bg-neutral-900 text-white dark:bg-white/[0.09] dark:text-white'
                    : 'text-neutral-600 hover:bg-neutral-100 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-white/[0.04] dark:hover:text-neutral-200'
                }`}
              >
                <Icon className="h-4 w-4 stroke-[1.75]" />
                <span>{item.label}</span>
              </button>
            );
          })}
        </nav>

        {/* Connection & Anti-Cheat Status Footer */}
        <div className="space-y-2 border-t border-neutral-200/80 pt-3 dark:border-white/[0.07]">
          <div className="flex items-center justify-between rounded-lg bg-neutral-100/80 px-2.5 py-2 text-xs dark:bg-white/[0.03]">
            <span className="text-neutral-500">Daemon</span>
            <span className="flex items-center gap-1.5 font-medium">
              <span
                className={`h-1.5 w-1.5 rounded-full ${
                  connected ? 'bg-emerald-500' : 'bg-amber-500'
                }`}
              />
              {connected ? 'Actif' : 'Déconnecté'}
            </span>
          </div>
          <div className="flex items-center justify-between rounded-lg bg-neutral-100/80 px-2.5 py-2 text-xs dark:bg-white/[0.03]">
            <span className="text-neutral-500">Home Assistant</span>
            <span className="flex items-center gap-1.5 font-medium">
              <span
                className={`h-1.5 w-1.5 rounded-full ${
                  haConnectedCount > 0 ? 'bg-emerald-500' : 'bg-neutral-400'
                }`}
              />
              {haConnectedCount > 0 ? `${haConnectedCount} connecté` : 'En attente'}
            </span>
          </div>
        </div>
      </aside>

      {/* Main Content */}
      <main className="flex-1 overflow-y-auto px-8 py-7">
        {!snapshot ? (
          <div className="mx-auto mt-24 max-w-md rounded-xl border border-neutral-200 bg-white p-6 text-center dark:border-white/[0.08] dark:bg-[#121418]">
            <div className="mx-auto mb-3 flex h-10 w-10 items-center justify-center rounded-full bg-neutral-100 dark:bg-white/[0.06]">
              <Activity className="h-5 w-5 text-neutral-500" />
            </div>
            <h2 className="text-base font-semibold">Connexion à ctrlmated…</h2>
            <p className="mt-1.5 text-sm text-neutral-500">
              Assure-toi que le service <code className="tabular">ctrlmated</code> tourne localement sur le port{' '}
              <span className="tabular">9488</span>.
            </p>
          </div>
        ) : (
          <div className="mx-auto max-w-4xl space-y-6">
            {/* Top Bar */}
            <header className="flex items-center justify-between">
              <div>
                <h1 className="text-xl font-semibold tracking-tight">
                  {navItems.find((n) => n.id === tab)?.label}
                </h1>
                <p className="mt-0.5 text-xs text-neutral-500">
                  {snapshot.manifest.host.name} · {snapshot.manifest.host.os_version}
                </p>
              </div>
              <div className="flex items-center gap-2">
                <span className="inline-flex items-center gap-1.5 rounded-full border border-neutral-200 bg-white px-2.5 py-1 text-xs font-medium text-neutral-700 dark:border-white/[0.09] dark:bg-white/[0.04] dark:text-neutral-300">
                  <ShieldCheck className="h-3.5 w-3.5 text-emerald-500" />
                  {snapshot.manifest.host.security_mode === 'zero_kernel_safe'
                    ? 'Anti-Cheat Safe'
                    : 'PawnIO Deep Mode'}
                </span>
                <button
                  onClick={async () => {
                    const nextUnit: TemperatureUnit = unit === 'celsius' ? 'fahrenheit' : 'celsius';
                    const updated = await updateSettings({ temperature_unit: nextUnit });
                    setSnapshot(updated);
                  }}
                  className="tabular rounded-full border border-neutral-200 bg-white px-2.5 py-1 text-xs font-medium text-neutral-700 transition hover:bg-neutral-100 dark:border-white/[0.09] dark:bg-white/[0.04] dark:text-neutral-300 dark:hover:bg-white/[0.08]"
                >
                  {unit === 'celsius' ? '°C' : '°F'}
                </button>
              </div>
            </header>

            {snapshot.thermal_failsafe_active && (
              <div className="flex items-center gap-3 rounded-xl border border-amber-500/40 bg-amber-500/10 px-4 py-3 text-sm text-amber-700 dark:text-amber-300">
                <AlertTriangle className="h-4 w-4 shrink-0" />
                <span>
                  Sécurité thermique active : refroidissement forcé à 100 % jusqu’au retour sous les seuils.
                </span>
              </div>
            )}

            {/* TAB: OVERVIEW */}
            {tab === 'overview' && (
              <div className="space-y-6">
                <section className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
                  <MetricCard
                    label="Processeur"
                    value={
                      numVal('cpu_0_temperature') !== undefined
                        ? formatTemp(numVal('cpu_0_temperature'), unit)
                        : `${(numVal('cpu_0_load') ?? 0).toFixed(1)} %`
                    }
                    sub={`Charge ${(numVal('cpu_0_load') ?? 0).toFixed(0)} % · ${numVal('cpu_0_frequency') ?? 0} MHz`}
                    progress={numVal('cpu_0_load') ?? 0}
                  />
                  <MetricCard
                    label="GPU Température"
                    value={formatTemp(numVal('gpu_0_temperature'), unit)}
                    sub={`Charge ${numVal('gpu_0_load') ?? 0} % · ${(numVal('gpu_0_power') ?? 0).toFixed(0)} W`}
                    progress={numVal('gpu_0_load') ?? 0}
                  />
                  <MetricCard
                    label={
                      numVal('cooler_0_liquid_temperature') !== undefined
                        ? 'Watercooling AIO'
                        : 'Mémoire RAM'
                    }
                    value={
                      numVal('cooler_0_liquid_temperature') !== undefined
                        ? formatTemp(numVal('cooler_0_liquid_temperature'), unit)
                        : `${(numVal('memory_0_usage') ?? 0).toFixed(1)} %`
                    }
                    sub={
                      numVal('cooler_0_liquid_temperature') !== undefined
                        ? `Pompe ${numVal('cooler_0_pump_rpm') ?? 0} RPM · Fan ${numVal('cooler_0_fan_rpm') ?? 0} RPM`
                        : `${(numVal('memory_0_used') ?? 0).toFixed(1)} GiB utilisés`
                    }
                    progress={
                      numVal('cooler_0_pump_duty') !== undefined
                        ? numVal('cooler_0_pump_duty') ?? 0
                        : numVal('memory_0_usage') ?? 0
                    }
                  />
                  <MetricCard
                    label="Mémoire & Stockage"
                    value={`${(numVal('memory_0_usage') ?? 0).toFixed(1)} %`}
                    sub={
                      numVal('storage_1_temperature') !== undefined
                        ? `RAM ${(numVal('memory_0_used') ?? 0).toFixed(1)} GiB · SSD ${formatTemp(numVal('storage_1_temperature'), unit)}`
                        : `RAM ${(numVal('memory_0_used') ?? 0).toFixed(1)} GiB · Disque ${(numVal('storage_0_usage') ?? 0).toFixed(0)} %`
                    }
                    progress={numVal('memory_0_usage') ?? 0}
                  />
                </section>

                {/* Privacy & Quick Controls */}
                <section className="grid grid-cols-1 gap-4 md:grid-cols-2">
                  <Card title="Confidentialité & Session">
                    <div className="space-y-3">
                      <StatusRow
                        icon={<AppWindow className="h-4 w-4" />}
                        label="Application active"
                        active={boolVal('gaming_mode')}
                        activeText={`${textVal('active_application') || 'Jeu'} · Gaming`}
                        idleText={textVal('active_application') || 'Bureau'}
                      />
                      <StatusRow
                        icon={<Mic className="h-4 w-4" />}
                        label="Microphone"
                        active={boolVal('microphone_in_use')}
                        activeText="En cours d'utilisation"
                        idleText="Inactif"
                      />
                      <StatusRow
                        icon={<Video className="h-4 w-4" />}
                        label="Webcam"
                        active={boolVal('webcam_in_use')}
                        activeText="Active"
                        idleText="Inactive"
                      />
                      {boolVal('gaming_mode') && (
                        <div className="flex items-center gap-1.5 pt-1 text-[11px] text-emerald-600 dark:text-emerald-400">
                          <Gamepad2 className="h-3.5 w-3.5" />
                          <span>Mode Gaming détecté — remontée HA accélérée (2 s)</span>
                        </div>
                      )}
                    </div>
                  </Card>

                  <Card title="Contrôles rapides">
                    <div className="flex flex-wrap gap-2">
                      <QuickAction
                        icon={<Sun className="h-3.5 w-3.5" />}
                        label={lightVal('light_global')?.is_on ? 'Éteindre LEDs' : 'Allumer LEDs'}
                        onClick={async () => {
                          await setLight({
                            entity_id: 'light_global',
                            is_on: !lightVal('light_global')?.is_on,
                          });
                        }}
                      />
                      <QuickAction
                        icon={<Lock className="h-3.5 w-3.5" />}
                        label="Verrouiller"
                        onClick={async () => {
                          const r = await pressButton('power_lock');
                          notify(r.message);
                        }}
                      />
                      <QuickAction
                        icon={<MonitorOff className="h-3.5 w-3.5" />}
                        label="Éteindre écrans"
                        onClick={async () => {
                          const r = await pressButton('display_off');
                          notify(r.message);
                        }}
                      />
                    </div>
                  </Card>
                </section>
              </div>
            )}

            {/* TAB: HARDWARE */}
            {tab === 'hardware' && (
              <div className="space-y-5">
                {snapshot.conflicts.length > 0 && (
                  <Card title="Logiciels tiers détectés (conflits potentiels)">
                    <div className="space-y-2.5">
                      {snapshot.conflicts.map((c) => (
                        <div
                          key={c.id}
                          className="flex items-start justify-between gap-4 rounded-lg border border-amber-500/25 bg-amber-500/[0.06] px-3.5 py-2.5 text-xs"
                        >
                          <div>
                            <div className="font-medium text-neutral-900 dark:text-neutral-100">
                              {c.name} <span className="text-neutral-500">· {c.category}</span>
                            </div>
                            <div className="mt-0.5 text-neutral-500">{c.impact}</div>
                          </div>
                          <span className="shrink-0 rounded-full bg-amber-500/15 px-2 py-0.5 text-[11px] font-medium text-amber-600 dark:text-amber-300">
                            En cours
                          </span>
                        </div>
                      ))}
                    </div>
                  </Card>
                )}

                <Card title="Appareils enfants Home Assistant 2026.11 (Child Devices)">
                  <div className="divide-y divide-neutral-200/70 dark:divide-white/[0.06]">
                    {snapshot.manifest.child_devices.map((dev) => {
                      const ents = snapshot.manifest.entities.filter(
                        (e) => e.child_device_id === dev.id,
                      );
                      return (
                        <div key={dev.id} className="py-3 first:pt-0 last:pb-0">
                          <div className="flex items-center justify-between">
                            <div>
                              <div className="text-sm font-medium">{dev.name}</div>
                              <div className="text-xs text-neutral-500">
                                {dev.category.toUpperCase()}
                                {dev.manufacturer ? ` · ${dev.manufacturer}` : ''}
                              </div>
                            </div>
                            <span className="tabular text-xs text-neutral-500">
                              {ents.length} entité(s)
                            </span>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </Card>

                <Card title="Mode matériel & Compatibilité Anti-Cheat">
                  <div className="flex flex-col justify-between gap-4 sm:flex-row sm:items-center">
                    <div className="space-y-1 text-xs text-neutral-500">
                      <div className="font-medium text-neutral-900 dark:text-neutral-100">
                        {snapshot.manifest.host.security_mode === 'zero_kernel_safe'
                          ? 'Mode Zero-Kernel / Anti-Cheat Safe (Actif)'
                          : snapshot.deep_bridge_active
                            ? 'Mode Deep Hardware (LibreHardwareMonitor + PawnIO actif)'
                            : 'Mode Deep Hardware demandé (bridge en attente ou mis en pause)'}
                      </div>
                      <p>
                        En mode Anti-Cheat Safe, aucun pilote noyau n'est chargé (100 % compatible
                        Valorant, FACEIT, EAC). Le mode Deep Hardware ajoute les capteurs CPU Ryzen
                        SMU, Super I/O carte mère et RAM DDR5, avec pause automatique si FACEIT se
                        lance.
                      </p>
                    </div>
                    <div className="flex shrink-0 gap-2">
                      <button
                        onClick={async () => {
                          const next =
                            snapshot.manifest.host.security_mode === 'zero_kernel_safe'
                              ? 'deep_hardware_pawn_io'
                              : 'zero_kernel_safe';
                          const updated = await updateSettings({ security_mode: next });
                          setSnapshot(updated);
                          notify(
                            next === 'zero_kernel_safe'
                              ? 'Mode Anti-Cheat Safe activé'
                              : 'Mode Deep Hardware (PawnIO) activé',
                          );
                        }}
                        className="rounded-lg border border-neutral-200 px-3 py-1.5 text-xs font-medium transition hover:bg-neutral-100 dark:border-white/[0.1] dark:hover:bg-white/[0.06]"
                      >
                        {snapshot.manifest.host.security_mode === 'zero_kernel_safe'
                          ? 'Activer Deep Hardware'
                          : 'Revenir en Anti-Cheat Safe'}
                      </button>
                      <button
                        onClick={async () => {
                          const updated = await rescanHardware();
                          setSnapshot(updated);
                          notify('Détection USB HID & RGB actualisée');
                        }}
                        className="inline-flex items-center gap-1.5 rounded-lg border border-neutral-200 px-3 py-1.5 text-xs font-medium transition hover:bg-neutral-100 dark:border-white/[0.1] dark:hover:bg-white/[0.06]"
                      >
                        <RefreshCw className="h-3.5 w-3.5" />
                        Rescanner
                      </button>
                    </div>
                  </div>
                </Card>

                <Card title="Contrôleurs USB HID natifs détectés">
                  {snapshot.hid_devices.length === 0 ? (
                    <p className="text-xs text-neutral-500">
                      Aucun contrôleur RGB/AIO USB HID reconnu pour l'instant.
                    </p>
                  ) : (
                    <div className="space-y-2">
                      {snapshot.hid_devices.map((h) => (
                        <div
                          key={`${h.vendor_id}-${h.product_id}`}
                          className="flex items-center justify-between rounded-lg border border-neutral-200/70 px-3 py-2 text-xs dark:border-white/[0.06]"
                        >
                          <div>
                            <div className="font-medium">{h.product}</div>
                            <div className="text-neutral-500">{h.manufacturer}</div>
                          </div>
                          <span className="tabular text-neutral-500">
                            {h.vendor_id.toString(16).padStart(4, '0')}:
                            {h.product_id.toString(16).padStart(4, '0')} · {h.driver_hint}
                          </span>
                        </div>
                      ))}
                    </div>
                  )}
                </Card>
              </div>
            )}

            {/* TAB: LIGHTING */}
            {tab === 'lighting' && activeLight && (
              <div className="space-y-5">
                <div className="flex flex-wrap gap-2">
                  {lightZones.map((z) => (
                    <button
                      key={z.id}
                      onClick={() => setSelectedZone(z.id)}
                      className={`rounded-lg border px-3 py-1.5 text-xs font-medium transition ${
                        selectedZone === z.id
                          ? 'border-neutral-900 bg-neutral-900 text-white dark:border-white dark:bg-white dark:text-neutral-900'
                          : 'border-neutral-200 bg-white text-neutral-700 hover:bg-neutral-100 dark:border-white/[0.08] dark:bg-white/[0.03] dark:text-neutral-300'
                      }`}
                    >
                      {z.name}
                    </button>
                  ))}
                </div>

                <Card title="Contrôleurs d'éclairage physiques actifs">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex flex-wrap gap-2">
                      {snapshot.rgb_backends.length === 0 ? (
                        <span className="text-xs text-neutral-500">
                          Aucun contrôleur ouvert (ferme GIGABYTE Control Center / NZXT CAM puis
                          clique Rescanner).
                        </span>
                      ) : (
                        snapshot.rgb_backends.map((b) => (
                          <span
                            key={b}
                            className="inline-flex items-center gap-1.5 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-xs font-medium text-emerald-700 dark:text-emerald-300"
                          >
                            <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                            {b}
                          </span>
                        ))
                      )}
                    </div>
                    <button
                      onClick={async () => {
                        setSnapshot(await rescanHardware());
                        notify('Contrôleurs RGB rescannés');
                      }}
                      className="inline-flex items-center gap-1.5 rounded-lg border border-neutral-200 px-2.5 py-1 text-xs font-medium dark:border-white/[0.1]"
                    >
                      <RefreshCw className="h-3 w-3" />
                      Rescanner
                    </button>
                  </div>
                </Card>

                <Card title="Contrôle d'ambiance (Intensité · Chaleur Kelvin · Couleur)">
                  <div className="space-y-6">
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-3">
                        <div
                          className="h-9 w-9 rounded-full border border-black/10 transition-colors dark:border-white/15"
                          style={{
                            backgroundColor: !activeLight.is_on
                              ? '#22252b'
                              : activeLight.color_mode === 'color_temp'
                                ? kelvinToPreviewHex(activeLight.color_temp_kelvin)
                                : `rgb(${activeLight.rgb.r}, ${activeLight.rgb.g}, ${activeLight.rgb.b})`,
                          }}
                        />
                        <div>
                          <div className="text-sm font-medium">
                            {activeLight.is_on ? 'Allumé' : 'Éteint'}
                          </div>
                          <div className="tabular text-xs text-neutral-500">
                            {activeLight.color_mode === 'color_temp'
                              ? `${activeLight.color_temp_kelvin} K · ${Math.round((activeLight.brightness / 255) * 100)} %`
                              : `RGB(${activeLight.rgb.r}, ${activeLight.rgb.g}, ${activeLight.rgb.b}) · ${Math.round((activeLight.brightness / 255) * 100)} %`}
                          </div>
                        </div>
                      </div>
                      <button
                        onClick={() =>
                          setLight({ entity_id: selectedZone, is_on: !activeLight.is_on })
                        }
                        className="rounded-lg border border-neutral-200 px-3.5 py-1.5 text-xs font-medium transition hover:bg-neutral-100 dark:border-white/[0.1] dark:hover:bg-white/[0.06]"
                      >
                        {activeLight.is_on ? 'Éteindre' : 'Allumer'}
                      </button>
                    </div>

                    {/* Brightness */}
                    <div className="space-y-2">
                      <div className="flex justify-between text-xs">
                        <span className="text-neutral-500">Intensité</span>
                        <span className="tabular font-medium">
                          {Math.round((activeLight.brightness / 255) * 100)} %
                        </span>
                      </div>
                      <input
                        type="range"
                        min={0}
                        max={255}
                        value={activeLight.brightness}
                        onChange={(e) =>
                          setLight({
                            entity_id: selectedZone,
                            brightness: Number(e.target.value),
                            transition_ms: 150,
                          })
                        }
                        className="w-full"
                        style={{
                          background: 'linear-gradient(90deg, #27272a 0%, #f4f4f5 100%)',
                          borderRadius: '9999px',
                        }}
                      />
                    </div>

                    {/* Color Temperature (Kelvin 2000K - 6500K) */}
                    <div className="space-y-2">
                      <div className="flex justify-between text-xs">
                        <span className="text-neutral-500">
                          Chaleur (sync Philips Hue / Govee)
                        </span>
                        <span className="tabular font-medium">
                          {activeLight.color_temp_kelvin} K
                        </span>
                      </div>
                      <input
                        type="range"
                        min={2000}
                        max={6500}
                        step={50}
                        value={activeLight.color_temp_kelvin}
                        onChange={(e) =>
                          setLight({
                            entity_id: selectedZone,
                            color_temp_kelvin: Number(e.target.value),
                            transition_ms: 200,
                          })
                        }
                        className="w-full"
                        style={{
                          background:
                            'linear-gradient(90deg, #ff9d42 0%, #ffd9a8 45%, #f5f7ff 100%)',
                          borderRadius: '9999px',
                        }}
                      />
                      <div className="flex justify-between text-[11px] text-neutral-500">
                        <span>2000K · Bougie</span>
                        <span>3000K · Chaud</span>
                        <span>4500K · Neutre</span>
                        <span>6500K · Jour</span>
                      </div>
                    </div>

                    {/* Refined Solid Color Swatches */}
                    <div className="space-y-2">
                      <div className="text-xs text-neutral-500">Couleur directe</div>
                      <div className="flex flex-wrap items-center gap-2.5">
                        {COLOR_SWATCHES.map((s) => (
                          <button
                            key={s.name}
                            title={s.name}
                            onClick={() =>
                              setLight({
                                entity_id: selectedZone,
                                rgb: s.rgb,
                                is_on: true,
                                transition_ms: 350,
                              })
                            }
                            className="h-7 w-7 rounded-full border border-black/15 transition hover:scale-110 dark:border-white/15"
                            style={{ backgroundColor: `rgb(${s.rgb.r}, ${s.rgb.g}, ${s.rgb.b})` }}
                          />
                        ))}
                      </div>
                    </div>
                  </div>
                </Card>
              </div>
            )}

            {/* TAB: COOLING */}
            {tab === 'cooling' && (
              <div className="space-y-5">
                <Card title="Mode de refroidissement">
                  <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-3">
                    {(Object.keys(PRESET_LABELS) as CoolingPreset[]).map((preset) => {
                      const active = textVal('cooling_preset') === preset;
                      return (
                        <button
                          key={preset}
                          onClick={async () => {
                            const r = await setCoolingPreset(preset);
                            notify(r.message);
                          }}
                          className={`rounded-xl border p-3.5 text-left transition ${
                            active
                              ? 'border-neutral-900 bg-neutral-900 text-white dark:border-white dark:bg-white/[0.1] dark:text-white'
                              : 'border-neutral-200 bg-white hover:bg-neutral-50 dark:border-white/[0.07] dark:bg-white/[0.02] dark:hover:bg-white/[0.05]'
                          }`}
                        >
                          <div className="text-sm font-medium">{PRESET_LABELS[preset].label}</div>
                          <div
                            className={`mt-1 text-xs ${
                              active ? 'text-neutral-300' : 'text-neutral-500'
                            }`}
                          >
                            {PRESET_LABELS[preset].desc}
                          </div>
                        </button>
                      );
                    })}
                  </div>
                </Card>

                <Card title="Garde-fou thermique matériel (Watchdog)">
                  <p className="text-xs leading-relaxed text-neutral-500">
                    La pompe watercooling AIO ne peut jamais descendre sous{' '}
                    <span className="tabular font-medium text-neutral-800 dark:text-neutral-200">
                      50 %
                    </span>
                    . En cas de dépassement critique (CPU ≥ 88 °C, GPU ≥ 83 °C ou liquide ≥ 48 °C),
                    Ctrlmate bascule automatiquement à 100 % et remonte l'alerte{' '}
                    <code className="tabular">thermal_failsafe</code> dans Home Assistant.
                  </p>
                </Card>
              </div>
            )}

            {/* TAB: SYSTEM */}
            {tab === 'system' && (
              <div className="space-y-5">
                <Card title="Alimentation & Session">
                  <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3">
                    {[
                      { id: 'power_lock', label: 'Verrouiller', icon: Lock },
                      { id: 'display_off', label: 'Éteindre écrans', icon: MonitorOff },
                      { id: 'power_sleep', label: 'Veille', icon: Moon },
                      { id: 'power_hibernate', label: 'Hibernation', icon: HardDrive },
                      { id: 'power_restart', label: 'Redémarrer', icon: RefreshCw },
                      { id: 'power_shutdown', label: 'Éteindre', icon: Power },
                    ].map((act) => {
                      const Icon = act.icon;
                      return (
                        <button
                          key={act.id}
                          onClick={async () => {
                            const r = await pressButton(act.id);
                            notify(r.message);
                          }}
                          className="flex items-center gap-2 rounded-lg border border-neutral-200 px-3 py-2.5 text-xs font-medium transition hover:bg-neutral-100 dark:border-white/[0.08] dark:hover:bg-white/[0.05]"
                        >
                          <Icon className="h-4 w-4 text-neutral-500" />
                          <span>{act.label}</span>
                        </button>
                      );
                    })}
                  </div>
                </Card>

                <Card title="Audio (WASAPI / PipeWire)">
                  <div className="space-y-4">
                    <div className="flex items-center gap-4">
                      <button
                        onClick={() =>
                          sendCommand({
                            action: 'set_switch',
                            entity_id: 'audio_mute',
                            state: !boolVal('audio_mute'),
                          })
                        }
                        className="rounded-lg border border-neutral-200 p-2 dark:border-white/[0.08]"
                      >
                        {boolVal('audio_mute') ? (
                          <VolumeX className="h-4 w-4 text-neutral-500" />
                        ) : (
                          <Volume2 className="h-4 w-4" />
                        )}
                      </button>
                      <input
                        type="range"
                        min={0}
                        max={100}
                        value={numVal('master_volume') ?? 50}
                        onChange={(e) =>
                          sendCommand({
                            action: 'set_number',
                            entity_id: 'master_volume',
                            value: Number(e.target.value),
                          })
                        }
                        className="flex-1"
                        style={{
                          background: 'linear-gradient(90deg, #52525b 0%, #e4e4e7 100%)',
                          borderRadius: '9999px',
                        }}
                      />
                      <span className="tabular w-11 text-right text-xs font-medium">
                        {Math.round(numVal('master_volume') ?? 50)} %
                      </span>
                    </div>

                    <div className="flex flex-col gap-1.5 sm:flex-row sm:items-center sm:justify-between">
                      <span className="text-xs text-neutral-500">Sortie audio par défaut</span>
                      <select
                        value={textVal('audio_output')}
                        onChange={async (e) => {
                          const r = await sendCommand({
                            action: 'set_select',
                            entity_id: 'audio_output',
                            option: e.target.value,
                          });
                          notify(r.message);
                        }}
                        className="rounded-lg border border-neutral-200 bg-white px-3 py-1.5 text-xs font-medium text-neutral-900 dark:border-white/[0.1] dark:bg-[#17191f] dark:text-neutral-100"
                      >
                        {audioOutputs.map((out) => (
                          <option key={out} value={out}>
                            {out}
                          </option>
                        ))}
                      </select>
                    </div>
                  </div>
                </Card>

                <Card title="Mises à jour système (Windows Update / PackageKit)">
                  {(() => {
                    const upd = updateVal();
                    const pending = upd?.pending_count ?? 0;
                    return (
                      <div className="space-y-3">
                        <div className="flex flex-wrap items-center justify-between gap-3">
                          <div>
                            <div className="text-sm font-medium">
                              {upd?.in_progress
                                ? 'Installation en cours…'
                                : pending > 0
                                  ? `${pending} mise(s) à jour en attente`
                                  : 'Système à jour'}
                            </div>
                            <div className="mt-0.5 text-xs text-neutral-500">
                              {upd?.installed_version ?? snapshot.manifest.host.os_version}
                              {upd?.reboot_required ? ' · Redémarrage requis' : ''}
                            </div>
                          </div>
                          <div className="flex gap-2">
                            <button
                              onClick={async () => {
                                const r = await sendCommand({ action: 'check_os_updates' });
                                notify(r.message);
                              }}
                              className="inline-flex items-center gap-1.5 rounded-lg border border-neutral-200 px-3 py-1.5 text-xs font-medium transition hover:bg-neutral-100 dark:border-white/[0.1] dark:hover:bg-white/[0.06]"
                            >
                              <RefreshCw className="h-3.5 w-3.5" />
                              Vérifier
                            </button>
                            {pending > 0 && (
                              <button
                                disabled={upd?.in_progress}
                                onClick={async () => {
                                  const r = await sendCommand({ action: 'install_os_updates' });
                                  notify(r.message);
                                }}
                                className="inline-flex items-center gap-1.5 rounded-lg bg-neutral-900 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-neutral-800 disabled:opacity-50 dark:bg-white dark:text-neutral-900"
                              >
                                <Download className="h-3.5 w-3.5" />
                                Installer
                              </button>
                            )}
                            <button
                              onClick={async () => {
                                const r = await sendCommand({
                                  action: 'send_notification',
                                  title: 'Ctrlmate',
                                  message: 'Notification bureau opérationnelle.',
                                });
                                notify(r.message);
                              }}
                              className="inline-flex items-center gap-1.5 rounded-lg border border-neutral-200 px-3 py-1.5 text-xs font-medium transition hover:bg-neutral-100 dark:border-white/[0.1] dark:hover:bg-white/[0.06]"
                            >
                              <Bell className="h-3.5 w-3.5" />
                              Test notif
                            </button>
                          </div>
                        </div>

                        {upd && upd.pending_titles.length > 0 && (
                          <ul className="space-y-1 border-t border-neutral-200/70 pt-3 text-xs text-neutral-500 dark:border-white/[0.07]">
                            {upd.pending_titles.map((t, i) => (
                              <li key={i} className="truncate">
                                • {t}
                              </li>
                            ))}
                          </ul>
                        )}
                      </div>
                    );
                  })()}
                </Card>

                <Card title="Scripts personnalisés (liste blanche → boutons Home Assistant)">
                  <div className="space-y-4">
                    <div className="grid grid-cols-1 gap-2 sm:grid-cols-4">
                      <input
                        value={newScriptName}
                        onChange={(e) => setNewScriptName(e.target.value)}
                        placeholder="Nom du bouton"
                        className="rounded-lg border border-neutral-200 bg-transparent px-3 py-1.5 text-xs dark:border-white/[0.1]"
                      />
                      <input
                        value={newScriptExe}
                        onChange={(e) => setNewScriptExe(e.target.value)}
                        placeholder="Exécutable (ex. notepad.exe)"
                        className="rounded-lg border border-neutral-200 bg-transparent px-3 py-1.5 text-xs dark:border-white/[0.1]"
                      />
                      <input
                        value={newScriptArgs}
                        onChange={(e) => setNewScriptArgs(e.target.value)}
                        placeholder="Arguments (optionnel)"
                        className="rounded-lg border border-neutral-200 bg-transparent px-3 py-1.5 text-xs dark:border-white/[0.1]"
                      />
                      <button
                        onClick={async () => {
                          if (!newScriptName.trim() || !newScriptExe.trim()) return;
                          const slug = newScriptName
                            .toLowerCase()
                            .replace(/[^a-z0-9]+/g, '_')
                            .replace(/^_|_$/g, '');
                          const nextItem: AllowlistedScript = {
                            id: `${slug || 'cmd'}_${Date.now().toString(36).slice(-4)}`,
                            name: newScriptName.trim(),
                            executable: newScriptExe.trim(),
                            args: newScriptArgs.trim() ? newScriptArgs.trim().split(/\s+/) : [],
                            enabled: true,
                          };
                          const updated = await updateSettings({
                            scripts: [...snapshot.scripts, nextItem],
                          });
                          setSnapshot(updated);
                          setNewScriptName('');
                          setNewScriptExe('');
                          setNewScriptArgs('');
                          notify(`Script « ${nextItem.name} » ajouté`);
                        }}
                        className="inline-flex items-center justify-center gap-1.5 rounded-lg bg-neutral-900 px-3 py-1.5 text-xs font-medium text-white dark:bg-white dark:text-neutral-900"
                      >
                        <Plus className="h-3.5 w-3.5" />
                        Ajouter
                      </button>
                    </div>

                    {snapshot.scripts.length === 0 ? (
                      <p className="text-xs text-neutral-500">
                        Aucun script autorisé. Chaque script ajouté ici devient un bouton dans Home
                        Assistant (exécuté uniquement en session utilisateur standard).
                      </p>
                    ) : (
                      <div className="space-y-2">
                        {snapshot.scripts.map((sc) => (
                          <div
                            key={sc.id}
                            className="flex items-center justify-between gap-3 rounded-lg border border-neutral-200/70 px-3 py-2 text-xs dark:border-white/[0.07]"
                          >
                            <div className="min-w-0">
                              <div className="font-medium">{sc.name}</div>
                              <div className="tabular truncate text-neutral-500">
                                {sc.executable} {sc.args.join(' ')}
                              </div>
                            </div>
                            <div className="flex shrink-0 items-center gap-2">
                              <button
                                onClick={async () => {
                                  const r = await pressButton(`script_${sc.id}`);
                                  notify(r.message);
                                }}
                                title="Exécuter"
                                className="rounded-md border border-neutral-200 p-1.5 hover:bg-neutral-100 dark:border-white/[0.1] dark:hover:bg-white/[0.06]"
                              >
                                <Play className="h-3.5 w-3.5" />
                              </button>
                              <button
                                onClick={async () => {
                                  const next = snapshot.scripts.filter((x) => x.id !== sc.id);
                                  setSnapshot(await updateSettings({ scripts: next }));
                                }}
                                title="Supprimer"
                                className="rounded-md p-1.5 text-neutral-500 hover:text-red-500"
                              >
                                <Trash2 className="h-3.5 w-3.5" />
                              </button>
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                </Card>
              </div>
            )}

            {/* TAB: HOME ASSISTANT */}
            {tab === 'homeassistant' && (
              <div className="space-y-5">
                <Card title="Appairage Home Assistant 2026.11 (Code PIN 6 chiffres · TLS 1.3)">
                  <div className="flex flex-col justify-between gap-4 sm:flex-row sm:items-center">
                    <div className="space-y-1">
                      <p className="text-xs text-neutral-500">
                        Découverte automatique mDNS sur le port{' '}
                        <span className="tabular font-medium text-neutral-800 dark:text-neutral-200">
                          {snapshot.pairing.listen_port}
                        </span>
                        . Empreinte TLS :{' '}
                        <span className="tabular">
                          {snapshot.pairing.cert_fingerprint.slice(0, 16)}…
                        </span>
                      </p>
                      {snapshot.pairing.active && snapshot.pairing.pin && (
                        <div className="mt-3 inline-flex items-center gap-3 rounded-xl border border-neutral-200 bg-neutral-100 px-4 py-2.5 dark:border-white/[0.1] dark:bg-white/[0.04]">
                          <KeyRound className="h-4 w-4 text-neutral-500" />
                          <span className="tabular text-2xl font-semibold tracking-[0.25em]">
                            {snapshot.pairing.pin}
                          </span>
                        </div>
                      )}
                    </div>

                    <div className="flex gap-2">
                      {snapshot.pairing.active ? (
                        <button
                          onClick={async () => setSnapshot(await cancelPairing())}
                          className="rounded-lg border border-neutral-200 px-3.5 py-2 text-xs font-medium dark:border-white/[0.1]"
                        >
                          Annuler
                        </button>
                      ) : (
                        <button
                          onClick={async () => setSnapshot(await startPairing())}
                          className="rounded-lg bg-neutral-900 px-4 py-2 text-xs font-medium text-white transition hover:bg-neutral-800 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200"
                        >
                          Démarrer l'appairage
                        </button>
                      )}
                    </div>
                  </div>

                  {snapshot.paired_clients.length > 0 && (
                    <div className="mt-5 space-y-2 border-t border-neutral-200/70 pt-4 dark:border-white/[0.07]">
                      <div className="text-xs font-medium text-neutral-500">
                        Instances appairées
                      </div>
                      {snapshot.paired_clients.map((cl) => (
                        <div
                          key={cl.client_id}
                          className="flex items-center justify-between rounded-lg border border-neutral-200/70 px-3 py-2 text-xs dark:border-white/[0.07]"
                        >
                          <div className="flex items-center gap-2">
                            <span
                              className={`h-2 w-2 rounded-full ${
                                cl.connected ? 'bg-emerald-500' : 'bg-neutral-400'
                              }`}
                            />
                            <span className="font-medium">{cl.client_name}</span>
                          </div>
                          <button
                            onClick={async () => setSnapshot(await revokeClient(cl.client_id))}
                            className="inline-flex items-center gap-1 text-neutral-500 hover:text-red-500"
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                            Révoquer
                          </button>
                        </div>
                      ))}
                    </div>
                  )}
                </Card>

                <Card title="Mode de remontée vers Home Assistant">
                  <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                    {(
                      [
                        ['intelligent', 'Intelligent'],
                        ['balanced', 'Équilibré'],
                        ['eco', 'Éco'],
                        ['realtime', 'Temps réel'],
                      ] as [PollingProfile, string][]
                    ).map(([prof, label]) => (
                      <button
                        key={prof}
                        onClick={async () =>
                          setSnapshot(await updateSettings({ polling_profile: prof }))
                        }
                        className={`rounded-lg border px-3 py-2 text-xs font-medium transition ${
                          snapshot.polling_profile === prof
                            ? 'border-neutral-900 bg-neutral-900 text-white dark:border-white dark:bg-white dark:text-neutral-900'
                            : 'border-neutral-200 dark:border-white/[0.08]'
                        }`}
                      >
                        {label}
                      </button>
                    ))}
                  </div>
                </Card>

                <Card title="Permissions accordées à Home Assistant">
                  <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                    {(
                      [
                        ['telemetry', 'Télémétrie matériel'],
                        ['lighting', 'Éclairage RGB & Kelvin'],
                        ['cooling', 'Modes refroidissement'],
                        ['audio_and_media', 'Audio & Volume'],
                        ['power_control', 'Alimentation (Éteindre / Redémarrer)'],
                        ['os_updates', 'Mises à jour OS'],
                        ['notifications', 'Notifications bureau'],
                        ['custom_scripts', 'Scripts personnalisés (allowlist)'],
                      ] as [keyof RemotePermissions, string][]
                    ).map(([key, label]) => (
                      <label
                        key={key}
                        className="flex cursor-pointer items-center justify-between rounded-lg border border-neutral-200/70 px-3 py-2.5 text-xs dark:border-white/[0.07]"
                      >
                        <span>{label}</span>
                        <input
                          type="checkbox"
                          checked={snapshot.permissions[key]}
                          onChange={async (e) => {
                            const next = { ...snapshot.permissions, [key]: e.target.checked };
                            setSnapshot(await updateSettings({ permissions: next }));
                          }}
                          className="h-4 w-4 accent-neutral-900 dark:accent-white"
                        />
                      </label>
                    ))}
                  </div>
                </Card>
              </div>
            )}
          </div>
        )}

        {toast && (
          <div className="fixed right-6 bottom-6 rounded-lg border border-neutral-200 bg-white px-4 py-2.5 text-xs font-medium shadow-lg dark:border-white/[0.1] dark:bg-[#17191f]">
            {toast}
          </div>
        )}
      </main>
    </div>
  );
}

function Card({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="rounded-xl border border-neutral-200/80 bg-white p-5 dark:border-white/[0.07] dark:bg-[#121418]">
      <h2 className="mb-4 text-xs font-semibold tracking-wide text-neutral-500 uppercase">
        {title}
      </h2>
      {children}
    </div>
  );
}

function MetricCard({
  label,
  value,
  sub,
  progress,
}: {
  label: string;
  value: string;
  sub: string;
  progress: number;
}) {
  return (
    <div className="rounded-xl border border-neutral-200/80 bg-white p-4 dark:border-white/[0.07] dark:bg-[#121418]">
      <div className="text-xs text-neutral-500">{label}</div>
      <div className="tabular mt-2 text-2xl font-semibold tracking-tight">{value}</div>
      <div className="tabular mt-1 text-[11px] text-neutral-500">{sub}</div>
      <div className="mt-3 h-1 w-full overflow-hidden rounded-full bg-neutral-100 dark:bg-white/[0.06]">
        <div
          className="h-full rounded-full bg-neutral-800 transition-all duration-300 dark:bg-neutral-300"
          style={{ width: `${Math.max(0, Math.min(100, progress))}%` }}
        />
      </div>
    </div>
  );
}

function StatusRow({
  icon,
  label,
  active,
  activeText,
  idleText,
}: {
  icon: React.ReactNode;
  label: string;
  active: boolean;
  activeText: string;
  idleText: string;
}) {
  return (
    <div className="flex items-center justify-between text-xs">
      <div className="flex items-center gap-2 text-neutral-600 dark:text-neutral-300">
        {icon}
        <span>{label}</span>
      </div>
      <span
        className={`rounded-full px-2 py-0.5 text-[11px] font-medium ${
          active
            ? 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400'
            : 'bg-neutral-100 text-neutral-500 dark:bg-white/[0.05]'
        }`}
      >
        {active ? activeText : idleText}
      </span>
    </div>
  );
}

function QuickAction({
  icon,
  label,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className="inline-flex items-center gap-1.5 rounded-lg border border-neutral-200 px-3 py-2 text-xs font-medium transition hover:bg-neutral-100 dark:border-white/[0.08] dark:hover:bg-white/[0.05]"
    >
      {icon}
      <span>{label}</span>
    </button>
  );
}
