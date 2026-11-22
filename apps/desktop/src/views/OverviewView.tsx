import {
  AppWindow,
  ArrowUpRight,
  Cpu,
  Droplets,
  Fan,
  Gamepad2,
  HardDrive,
  Home,
  Lock,
  Mic,
  MonitorOff,
  Sun,
  Video,
  Volume2,
  VolumeX,
} from 'lucide-react';
import {
  formatTemp,
  kelvinToPreviewHex,
  pressButton,
  sendCommand,
  setCoolingPreset,
  setLight,
} from '../api';
import {
  Sparkline,
  StatusBadge,
  SurfaceCard,
  ThermalBar,
} from '../components/Primitives';
import type { NavTab } from '../components/CommandPalette';
import type { TelemetryHistory } from '../hooks/useTelemetryHistory';
import { useSnapshotHelpers } from '../hooks/useTelemetryHistory';
import type { CoolingPreset, DaemonSnapshot, TemperatureUnit } from '../types';

const QUICK_COOLING: { id: CoolingPreset; label: string }[] = [
  { id: 'auto', label: 'Auto' },
  { id: 'silent', label: 'Silence' },
  { id: 'balanced', label: 'Équilibré' },
  { id: 'performance', label: 'Perf' },
  { id: 'full_speed', label: 'Max' },
];

const QUICK_KELVIN: { k: number; label: string }[] = [
  { k: 2700, label: '2700K' },
  { k: 4000, label: '4000K' },
  { k: 5500, label: '5500K' },
];

export function OverviewView({
  snapshot,
  history,
  unit,
  onNavigate,
  onNotify,
}: {
  snapshot: DaemonSnapshot;
  history: TelemetryHistory;
  unit: TemperatureUnit;
  onNavigate: (tab: NavTab) => void;
  onNotify: (msg: string) => void;
}) {
  const { numVal, boolVal, textVal, lightVal, attrVal, primarySsd } =
    useSnapshotHelpers(snapshot);

  const cpuTemp = numVal('cpu_0_temperature');
  const cpuLoad = numVal('cpu_0_load') ?? 0;
  const cpuMhz = numVal('cpu_0_frequency') ?? 0;

  const gpuTemp = numVal('gpu_0_temperature');
  const gpuLoad = numVal('gpu_0_load') ?? 0;
  const gpuPower = numVal('gpu_0_power') ?? 0;
  const gpuFan = numVal('gpu_0_fan_speed');
  const vramUsed = attrVal('gpu_0_vram_usage', 'vram_used_mib') as number | undefined;
  const vramTotal = attrVal('gpu_0_vram_usage', 'vram_total_mib') as number | undefined;

  const liquidTemp = numVal('cooler_0_liquid_temperature');
  const pumpRpm = numVal('cooler_0_pump_rpm');
  const pumpDuty = numVal('cooler_0_pump_duty');
  const fanRpm = numVal('cooler_0_fan_rpm');
  const fanDuty = numVal('cooler_0_fan_duty');

  const ramUsage = numVal('memory_0_usage') ?? 0;
  const ramUsed = numVal('memory_0_used') ?? 0;

  const globalLight = lightVal('light_global');
  const activePreset = (textVal('cooling_preset') || 'auto') as CoolingPreset;
  const gamingMode = boolVal('gaming_mode');
  const micActive = boolVal('microphone_in_use');
  const camActive = boolVal('webcam_in_use');
  const activeApp = textVal('active_application') || 'Bureau';
  const muted = boolVal('audio_mute');
  const volume = numVal('master_volume') ?? 50;
  const audioOut = textVal('audio_output');
  const audioOutputs =
    snapshot.manifest.entities.find((e) => e.id === 'audio_output')?.options ?? [];

  const haConnected = snapshot.paired_clients.filter((c) => c.connected).length;

  const lightPreviewCss = !globalLight?.is_on
    ? '#23242a'
    : globalLight.color_mode === 'color_temp'
      ? kelvinToPreviewHex(globalLight.color_temp_kelvin)
      : `rgb(${globalLight.rgb.r}, ${globalLight.rgb.g}, ${globalLight.rgb.b})`;

  return (
    <div className="view-enter space-y-5">
      {/* Top Architectural Status Strip */}
      <div className="surface-card flex flex-wrap items-center justify-between gap-3 rounded-xl px-4 py-3">
        <div className="flex flex-wrap items-center gap-2.5">
          <span className="flex items-center gap-1.5 text-xs font-medium text-[var(--text-primary)]">
            <AppWindow className="h-3.5 w-3.5 text-[var(--text-muted)]" />
            <span>{activeApp}</span>
          </span>
          <span className="h-3.5 w-[1px] bg-[var(--border-hairline)]" />
          {gamingMode ? (
            <StatusBadge tone="emerald" pulse>
              <Gamepad2 className="h-3 w-3" />
              Mode Gaming · 2 s
            </StatusBadge>
          ) : (
            <StatusBadge tone="neutral" dot={false}>
              Session bureau · Intelligent
            </StatusBadge>
          )}
          <StatusBadge tone={micActive ? 'amber' : 'neutral'} pulse={micActive}>
            <Mic className="h-3 w-3" />
            {micActive ? 'Micro actif' : 'Micro inactif'}
          </StatusBadge>
          <StatusBadge tone={camActive ? 'red' : 'neutral'} pulse={camActive}>
            <Video className="h-3 w-3" />
            {camActive ? 'Caméra active' : 'Caméra inactive'}
          </StatusBadge>
        </div>

        <div className="flex items-center gap-1.5">
          <button
            type="button"
            onClick={async () => {
              const r = await pressButton('power_lock');
              onNotify(r.message);
            }}
            className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium text-[var(--text-secondary)] ring-1 ring-black/[0.07] transition hover:bg-black/[0.04] hover:text-[var(--text-primary)] dark:ring-white/[0.08] dark:hover:bg-white/[0.06]"
          >
            <Lock className="h-3 w-3" />
            Verrouiller
          </button>
          <button
            type="button"
            onClick={async () => {
              const r = await pressButton('display_off');
              onNotify(r.message);
            }}
            className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium text-[var(--text-secondary)] ring-1 ring-black/[0.07] transition hover:bg-black/[0.04] hover:text-[var(--text-primary)] dark:ring-white/[0.08] dark:hover:bg-white/[0.06]"
          >
            <MonitorOff className="h-3 w-3" />
            Écrans off
          </button>
        </div>
      </div>

      {/* 4-Column Bento Telemetry Grid with Live 60s Sparklines */}
      <div className="grid grid-cols-1 gap-3.5 sm:grid-cols-2 lg:grid-cols-4">
        {/* CPU Card */}
        <TelemetryBentoCard
          icon={<Cpu className="h-3.5 w-3.5" />}
          label="Processeur"
          hero={cpuTemp !== undefined ? formatTemp(cpuTemp, unit) : `${cpuLoad.toFixed(1)} %`}
          meta={
            cpuTemp !== undefined
              ? `${cpuLoad.toFixed(0)} % · ${cpuMhz} MHz`
              : `${cpuMhz} MHz · Charge`
          }
          percent={cpuLoad}
          series={cpuTemp !== undefined ? history.cpuTemp : history.cpuLoad}
          stroke={cpuTemp !== undefined && cpuTemp >= 80 ? '#d97706' : '#a1a1aa'}
          onClick={() => onNavigate('hardware')}
        />

        {/* GPU Card */}
        <TelemetryBentoCard
          icon={<Gamepad2 className="h-3.5 w-3.5" />}
          label="GPU"
          hero={formatTemp(gpuTemp, unit)}
          meta={`${gpuLoad} % · ${gpuPower.toFixed(0)} W${
            gpuFan !== undefined ? ` · Fan ${gpuFan} %` : ''
          }`}
          extra={
            vramUsed !== undefined && vramTotal !== undefined
              ? `VRAM ${(vramUsed / 1024).toFixed(1)} / ${(vramTotal / 1024).toFixed(0)} Go`
              : undefined
          }
          percent={gpuLoad}
          series={history.gpuTemp}
          stroke={gpuTemp !== undefined && gpuTemp >= 78 ? '#d97706' : '#60a5fa'}
          onClick={() => onNavigate('hardware')}
        />

        {/* AIO Liquid Cooling Card */}
        <TelemetryBentoCard
          icon={<Droplets className="h-3.5 w-3.5" />}
          label={liquidTemp !== undefined ? 'Watercooling AIO' : 'Refroidissement'}
          hero={liquidTemp !== undefined ? formatTemp(liquidTemp, unit) : activePreset.toUpperCase()}
          meta={
            pumpRpm !== undefined
              ? `Pompe ${pumpRpm} RPM (${pumpDuty ?? 0} %)`
              : 'Mode de ventilation'
          }
          extra={
            fanRpm !== undefined
              ? `Radiateur ${fanRpm} RPM (${fanDuty ?? 0} %)`
              : undefined
          }
          percent={pumpDuty ?? 50}
          series={history.liquidTemp}
          stroke={liquidTemp !== undefined && liquidTemp >= 44 ? '#d97706' : '#34d399'}
          onClick={() => onNavigate('cooling')}
        />

        {/* Memory & NVMe Card */}
        <TelemetryBentoCard
          icon={<HardDrive className="h-3.5 w-3.5" />}
          label="Mémoire & NVMe"
          hero={`${ramUsage.toFixed(1)} %`}
          meta={`${ramUsed.toFixed(1)} GiB utilisés`}
          extra={
            primarySsd
              ? `SSD ${
                  primarySsd.temp !== undefined ? formatTemp(primarySsd.temp, unit) : ''
                }${primarySsd.usage !== undefined ? ` · ${primarySsd.usage.toFixed(0)} %` : ''}`
              : undefined
          }
          percent={ramUsage}
          series={history.ramUsage}
          stroke="#a1a1aa"
          onClick={() => onNavigate('hardware')}
        />
      </div>

      {/* Lower 2-Column Precision Control Bento */}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-12">
        {/* Left 7 cols: Lighting & Cooling Express Studio */}
        <div className="space-y-4 lg:col-span-7">
          <SurfaceCard
            title="Ambiance lumineuse"
            action={
              <button
                type="button"
                onClick={() => onNavigate('lighting')}
                className="inline-flex items-center gap-1 text-[11px] font-medium text-[var(--text-muted)] transition hover:text-[var(--text-primary)]"
              >
                Studio éclairage
                <ArrowUpRight className="h-3 w-3" />
              </button>
            }
          >
            <div className="space-y-4">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div className="flex items-center gap-3">
                  <button
                    type="button"
                    onClick={() =>
                      setLight({
                        entity_id: 'light_global',
                        is_on: !globalLight?.is_on,
                      })
                    }
                    title="Allumer / Éteindre"
                    className="relative flex h-10 w-10 items-center justify-center rounded-xl ring-1 ring-black/10 transition hover:scale-[1.03] dark:ring-white/15"
                    style={{
                      backgroundColor: lightPreviewCss,
                      boxShadow: globalLight?.is_on
                        ? `0 0 24px -4px ${lightPreviewCss}`
                        : undefined,
                    }}
                  >
                    <Sun
                      className={`h-4 w-4 ${
                        globalLight?.is_on ? 'text-neutral-900/80' : 'text-neutral-400'
                      }`}
                    />
                  </button>
                  <div>
                    <div className="text-sm font-medium">
                      {globalLight?.is_on ? 'Éclairage PC actif' : 'Éclairage éteint'}
                    </div>
                    <div className="tabular text-xs text-[var(--text-muted)]">
                      {globalLight
                        ? globalLight.color_mode === 'color_temp'
                          ? `${globalLight.color_temp_kelvin} K · ${Math.round((globalLight.brightness / 255) * 100)} %`
                          : `RGB(${globalLight.rgb.r}, ${globalLight.rgb.g}, ${globalLight.rgb.b}) · ${Math.round((globalLight.brightness / 255) * 100)} %`
                        : '—'}
                      {' · '}
                      {snapshot.rgb_backends.length} contrôleur(s)
                    </div>
                  </div>
                </div>

                <div className="flex items-center gap-1.5">
                  {QUICK_KELVIN.map((p) => {
                    const isCurrent =
                      globalLight?.is_on &&
                      globalLight.color_mode === 'color_temp' &&
                      Math.abs(globalLight.color_temp_kelvin - p.k) <= 150;
                    return (
                      <button
                        key={p.k}
                        type="button"
                        onClick={() =>
                          setLight({
                            entity_id: 'light_global',
                            is_on: true,
                            color_temp_kelvin: p.k,
                            transition_ms: 350,
                          })
                        }
                        className={`tabular inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium transition ${
                          isCurrent
                            ? 'bg-neutral-900 text-white dark:bg-white dark:text-neutral-950'
                            : 'bg-black/[0.04] text-[var(--text-secondary)] hover:bg-black/[0.08] dark:bg-white/[0.05] dark:hover:bg-white/[0.1]'
                        }`}
                      >
                        <span
                          className="h-2 w-2 rounded-full ring-1 ring-black/15"
                          style={{ backgroundColor: kelvinToPreviewHex(p.k) }}
                        />
                        {p.label}
                      </button>
                    );
                  })}
                </div>
              </div>

              {globalLight && (
                <div className="space-y-1.5">
                  <div className="flex items-center justify-between text-[11px] text-[var(--text-muted)]">
                    <span>Intensité globale</span>
                    <span className="tabular font-medium text-[var(--text-primary)]">
                      {Math.round((globalLight.brightness / 255) * 100)} %
                    </span>
                  </div>
                  <input
                    type="range"
                    min={0}
                    max={255}
                    value={globalLight.brightness}
                    onChange={(e) =>
                      setLight({
                        entity_id: 'light_global',
                        is_on: Number(e.target.value) > 0,
                        brightness: Number(e.target.value),
                        transition_ms: 140,
                      })
                    }
                    className="ctrl-slider w-full"
                    style={{
                      background: `linear-gradient(90deg, rgba(113,113,122,0.22) 0%, ${lightPreviewCss} 100%)`,
                      borderRadius: '9999px',
                    }}
                  />
                </div>
              )}
            </div>
          </SurfaceCard>

          <SurfaceCard
            title="Profil de refroidissement"
            action={
              <button
                type="button"
                onClick={() => onNavigate('cooling')}
                className="inline-flex items-center gap-1 text-[11px] font-medium text-[var(--text-muted)] transition hover:text-[var(--text-primary)]"
              >
                Courbes & sécurité
                <ArrowUpRight className="h-3 w-3" />
              </button>
            }
          >
            <div className="grid grid-cols-5 gap-1.5 rounded-xl bg-black/[0.035] p-1 dark:bg-white/[0.035]">
              {QUICK_COOLING.map((item) => {
                const active = activePreset === item.id;
                return (
                  <button
                    key={item.id}
                    type="button"
                    onClick={async () => {
                      const r = await setCoolingPreset(item.id);
                      onNotify(r.message);
                    }}
                    className={`rounded-lg py-2 text-xs font-medium transition ${
                      active
                        ? 'bg-white text-neutral-900 shadow-sm ring-1 ring-black/[0.06] dark:bg-white/[0.11] dark:text-white dark:ring-white/[0.1]'
                        : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)]'
                    }`}
                  >
                    {item.label}
                  </button>
                );
              })}
            </div>
            <div className="mt-3 flex items-center justify-between text-[11px] text-[var(--text-muted)]">
              <span className="flex items-center gap-1.5">
                <Fan className="h-3 w-3" />
                Garde-fou thermique actif · Pompe AIO protégée ≥ 50 %
              </span>
              <span className="tabular font-medium text-emerald-600 dark:text-emerald-400">
                {snapshot.thermal_failsafe_active ? 'URGENCE 100 %' : 'Nominal'}
              </span>
            </div>
          </SurfaceCard>
        </div>

        {/* Right 5 cols: Audio & Home Assistant Live Summary */}
        <div className="space-y-4 lg:col-span-5">
          <SurfaceCard
            title="Audio système"
            action={
              <button
                type="button"
                onClick={() => onNavigate('system')}
                className="inline-flex items-center gap-1 text-[11px] font-medium text-[var(--text-muted)] transition hover:text-[var(--text-primary)]"
              >
                Système
                <ArrowUpRight className="h-3 w-3" />
              </button>
            }
          >
            <div className="space-y-3.5">
              <div className="flex items-center gap-3">
                <button
                  type="button"
                  onClick={() =>
                    sendCommand({
                      action: 'set_switch',
                      entity_id: 'audio_mute',
                      state: !muted,
                    })
                  }
                  className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-lg ring-1 transition ${
                    muted
                      ? 'bg-amber-500/10 text-amber-600 ring-amber-500/25 dark:text-amber-400'
                      : 'bg-black/[0.03] text-[var(--text-primary)] ring-black/[0.07] hover:bg-black/[0.06] dark:bg-white/[0.04] dark:ring-white/[0.08]'
                  }`}
                >
                  {muted ? <VolumeX className="h-4 w-4" /> : <Volume2 className="h-4 w-4" />}
                </button>

                <div className="flex-1 space-y-1">
                  <div className="flex justify-between text-[11px]">
                    <span className="text-[var(--text-muted)]">Volume principal</span>
                    <span className="tabular font-medium">
                      {muted ? 'Muet' : `${Math.round(volume)} %`}
                    </span>
                  </div>
                  <input
                    type="range"
                    min={0}
                    max={100}
                    value={volume}
                    onChange={(e) =>
                      sendCommand({
                        action: 'set_number',
                        entity_id: 'master_volume',
                        value: Number(e.target.value),
                      })
                    }
                    className="ctrl-slider w-full"
                    style={{
                      background: `linear-gradient(90deg, rgba(161,161,170,0.85) ${volume}%, rgba(161,161,170,0.18) ${volume}%)`,
                      borderRadius: '9999px',
                    }}
                  />
                </div>
              </div>

              <select
                value={audioOut}
                onChange={async (e) => {
                  const r = await sendCommand({
                    action: 'set_select',
                    entity_id: 'audio_output',
                    option: e.target.value,
                  });
                  onNotify(r.message);
                }}
                className="w-full truncate rounded-lg bg-[var(--bg-subtle)] px-3 py-2 text-xs font-medium text-[var(--text-primary)] ring-1 ring-black/[0.07] dark:ring-white/[0.08]"
              >
                {audioOutputs.map((out) => (
                  <option key={out} value={out}>
                    {out}
                  </option>
                ))}
              </select>
            </div>
          </SurfaceCard>

          <SurfaceCard
            title="Home Assistant 2026.11"
            action={
              <button
                type="button"
                onClick={() => onNavigate('homeassistant')}
                className="inline-flex items-center gap-1 text-[11px] font-medium text-[var(--text-muted)] transition hover:text-[var(--text-primary)]"
              >
                Configurer
                <ArrowUpRight className="h-3 w-3" />
              </button>
            }
          >
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2.5">
                  <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-black/[0.04] dark:bg-white/[0.05]">
                    <Home className="h-4 w-4 text-[var(--text-secondary)]" />
                  </div>
                  <div>
                    <div className="text-xs font-medium">
                      {haConnected > 0
                        ? `${haConnected} instance connectée`
                        : 'En attente de connexion'}
                    </div>
                    <div className="tabular text-[11px] text-[var(--text-muted)]">
                      WSS TLS 1.3 · Port {snapshot.pairing.listen_port}
                    </div>
                  </div>
                </div>
                <StatusBadge tone={haConnected > 0 ? 'emerald' : 'neutral'} pulse={haConnected > 0}>
                  {haConnected > 0 ? 'En ligne' : 'Veille'}
                </StatusBadge>
              </div>

              <div className="hairline-t grid grid-cols-3 gap-2 pt-3 text-center">
                <div>
                  <div className="tabular text-sm font-semibold">
                    {snapshot.manifest.child_devices.length}
                  </div>
                  <div className="text-[10px] text-[var(--text-muted)]">Appareils</div>
                </div>
                <div>
                  <div className="tabular text-sm font-semibold">
                    {snapshot.manifest.entities.length}
                  </div>
                  <div className="text-[10px] text-[var(--text-muted)]">Entités</div>
                </div>
                <div>
                  <div className="tabular text-sm font-semibold capitalize">
                    {snapshot.polling_profile === 'intelligent'
                      ? 'Auto'
                      : snapshot.polling_profile}
                  </div>
                  <div className="text-[10px] text-[var(--text-muted)]">Cadence</div>
                </div>
              </div>
            </div>
          </SurfaceCard>
        </div>
      </div>
    </div>
  );
}

function TelemetryBentoCard({
  icon,
  label,
  hero,
  meta,
  extra,
  percent,
  series,
  stroke,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  hero: string;
  meta: string;
  extra?: string;
  percent: number;
  series: number[];
  stroke: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="surface-card surface-card-interactive group flex flex-col justify-between rounded-xl p-4 text-left"
    >
      <div className="flex items-center justify-between text-[11px] text-[var(--text-muted)]">
        <span className="flex items-center gap-1.5 font-medium">
          {icon}
          <span>{label}</span>
        </span>
        <ArrowUpRight className="h-3 w-3 opacity-0 transition group-hover:opacity-100" />
      </div>

      <div className="mt-2.5">
        <div className="tabular text-[26px] leading-none font-semibold tracking-[-0.03em]">
          {hero}
        </div>
        <div className="tabular mt-1.5 truncate text-[11px] text-[var(--text-secondary)]">
          {meta}
        </div>
        {extra && (
          <div className="tabular mt-0.5 truncate text-[10px] text-[var(--text-muted)]">
            {extra}
          </div>
        )}
      </div>

      <div className="mt-3 space-y-2">
        <Sparkline data={series} stroke={stroke} height={32} />
        <ThermalBar percent={percent} />
      </div>
    </button>
  );
}
