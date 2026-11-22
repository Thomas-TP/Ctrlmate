import {
  AlertTriangle,
  CircuitBoard,
  Cpu,
  Droplets,
  Gamepad2,
  HardDrive,
  MemoryStick,
  RefreshCw,
  ShieldCheck,
  ShieldHalf,
  Usb,
} from 'lucide-react';
import { formatTemp, rescanHardware, updateSettings } from '../api';
import { StatusBadge, SurfaceCard } from '../components/Primitives';
import { useSnapshotHelpers } from '../hooks/useTelemetryHistory';
import type { DaemonSnapshot, EntityDescriptor, SecurityMode, TemperatureUnit } from '../types';

function categoryIcon(category: string) {
  switch (category) {
    case 'cpu':
      return Cpu;
    case 'gpu':
      return Gamepad2;
    case 'memory':
      return MemoryStick;
    case 'storage':
      return HardDrive;
    case 'cooler':
      return Droplets;
    default:
      return CircuitBoard;
  }
}

const ENTITY_LABEL_FR: Record<string, string> = {
  Load: 'Charge',
  Frequency: 'Fréquence',
  Temperature: 'Température',
  Power: 'Puissance',
  Usage: 'Utilisation',
  Used: 'Utilisée',
  'VRAM Usage': 'VRAM',
  'Fan Speed': 'Ventilateur',
  'Core Clock': 'Horloge',
  'Space Used': 'Espace',
  'Liquid Temperature': 'Liquide',
  'Pump Speed': 'Pompe',
  'Pump Duty': 'Duty pompe',
  'Radiator Fans': 'Ventilateurs',
  'Radiator Fan Duty': 'Duty ventilos',
  'VRM Temperature': 'Temp. VRM',
  'Chipset Temperature': 'Chipset',
  Lighting: 'Éclairage',
};

function localizedEntityLabel(name: string): string {
  return ENTITY_LABEL_FR[name] ?? name;
}

export function HardwareView({
  snapshot,
  unit,
  onSnapshot,
  onNotify,
}: {
  snapshot: DaemonSnapshot;
  unit: TemperatureUnit;
  onSnapshot: (s: DaemonSnapshot) => void;
  onNotify: (msg: string) => void;
}) {
  const { stateMap } = useSnapshotHelpers(snapshot);
  const mode = snapshot.manifest.host.security_mode;

  const formatEntityReading = (ent: EntityDescriptor): string => {
    const st = stateMap.get(ent.id);
    if (!st) return '—';
    const v = st.value;
    if (v.kind === 'float' || v.kind === 'int') {
      if (ent.device_class === 'temperature' || ent.unit_of_measurement === '°C') {
        return formatTemp(v.value, unit);
      }
      const numStr = v.kind === 'float' ? v.value.toFixed(1) : String(v.value);
      return ent.unit_of_measurement ? `${numStr} ${ent.unit_of_measurement}` : numStr;
    }
    if (v.kind === 'bool') return v.value ? 'Actif' : 'Inactif';
    if (v.kind === 'light') {
      return v.value.is_on
        ? `${Math.round((v.value.brightness / 255) * 100)} % · ${v.value.color_temp_kelvin}K`
        : 'Éteint';
    }
    if (v.kind === 'text') return v.value;
    return '—';
  };

  const switchMode = async (next: SecurityMode) => {
    if (next === mode) return;
    const updated = await updateSettings({ security_mode: next });
    onSnapshot(updated);
    onNotify(
      next === 'zero_kernel_safe'
        ? 'Mode Anti-Cheat Safe activé (0 pilote noyau)'
        : 'Mode Deep Hardware (PawnIO) activé',
    );
  };

  return (
    <div className="view-enter space-y-5">
      {/* Conflicts alert if any */}
      {snapshot.conflicts.length > 0 && (
        <div className="rounded-xl bg-amber-500/[0.07] p-4 ring-1 ring-amber-500/25">
          <div className="mb-2.5 flex items-center gap-2 text-xs font-medium text-amber-700 dark:text-amber-300">
            <AlertTriangle className="h-3.5 w-3.5" />
            <span>Logiciels constructeur détectés ({snapshot.conflicts.length})</span>
          </div>
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
            {snapshot.conflicts.map((c) => (
              <div
                key={c.id}
                className="rounded-lg bg-[var(--bg-surface)] px-3 py-2.5 text-xs ring-1 ring-black/[0.06] dark:ring-white/[0.07]"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="font-medium">{c.name}</span>
                  <span className="text-[10px] text-[var(--text-muted)]">{c.category}</span>
                </div>
                <p className="mt-1 text-[11px] leading-relaxed text-[var(--text-muted)]">
                  {c.impact}
                </p>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Security & Anti-Cheat Mode Selector */}
      <SurfaceCard
        title="Architecture d'accès matériel & Anti-Cheat"
        action={
          <button
            type="button"
            onClick={async () => {
              const updated = await rescanHardware();
              onSnapshot(updated);
              onNotify('Matériel USB HID, NVMe & RGB rescanné');
            }}
            className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium text-[var(--text-secondary)] ring-1 ring-black/[0.08] transition hover:bg-black/[0.04] hover:text-[var(--text-primary)] dark:ring-white/[0.1] dark:hover:bg-white/[0.06]"
          >
            <RefreshCw className="h-3 w-3" />
            Rescanner le bus
          </button>
        }
      >
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          <button
            type="button"
            onClick={() => switchMode('zero_kernel_safe')}
            className={`rounded-xl p-4 text-left transition ${
              mode === 'zero_kernel_safe'
                ? 'bg-neutral-900/[0.04] ring-1.5 ring-neutral-900 dark:bg-white/[0.06] dark:ring-white'
                : 'bg-[var(--bg-subtle)]/50 ring-1 ring-[var(--border-hairline)] hover:ring-[var(--border-strong)]'
            }`}
          >
            <div className="flex items-center justify-between">
              <span className="flex items-center gap-2 text-xs font-semibold">
                <ShieldCheck className="h-4 w-4 text-emerald-500" />
                Zero-Kernel · Anti-Cheat Safe
              </span>
              {mode === 'zero_kernel_safe' && <StatusBadge tone="emerald">Actif</StatusBadge>}
            </div>
            <p className="mt-2 text-[11px] leading-relaxed text-[var(--text-secondary)]">
              Aucun pilote noyau Ring-0 chargé. 100 % compatible Valorant (Vanguard), FACEIT,
              Easy Anti-Cheat et HVCI. Utilise NVML, USB HID natif (Kraken, Gigabyte, NZXT) et
              NVMe IOCTL.
            </p>
          </button>

          <button
            type="button"
            onClick={() => switchMode('deep_hardware_pawn_io')}
            className={`rounded-xl p-4 text-left transition ${
              mode === 'deep_hardware_pawn_io'
                ? 'bg-neutral-900/[0.04] ring-1.5 ring-neutral-900 dark:bg-white/[0.06] dark:ring-white'
                : 'bg-[var(--bg-subtle)]/50 ring-1 ring-[var(--border-hairline)] hover:ring-[var(--border-strong)]'
            }`}
          >
            <div className="flex items-center justify-between">
              <span className="flex items-center gap-2 text-xs font-semibold">
                <ShieldHalf className="h-4 w-4 text-amber-500" />
                Deep Hardware · PawnIO (Opt-in)
              </span>
              {mode === 'deep_hardware_pawn_io' && (
                <StatusBadge tone={snapshot.deep_bridge_active ? 'emerald' : 'amber'}>
                  {snapshot.deep_bridge_active ? 'Bridge actif' : 'En attente'}
                </StatusBadge>
              )}
            </div>
            <p className="mt-2 text-[11px] leading-relaxed text-[var(--text-secondary)]">
              Active les capteurs avancés (Ryzen SMU, VRM carte mère, ventilateurs boîtier) et le
              SMBus G.Skill DDR5 via PawnIO, avec{' '}
              <span className="font-medium text-[var(--text-primary)]">
                pause automatique dès le lancement de FACEIT Anti-Cheat
              </span>
              .
            </p>
          </button>
        </div>
      </SurfaceCard>

      {/* Child Devices Grid with Live Telemetry Readings */}
      <SurfaceCard
        title="Composants détectés (Child Devices Home Assistant 2026.11)"
        subtitle="Tous les appareils enfants héritent automatiquement de la pièce du PC parent."
      >
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          {snapshot.manifest.child_devices.map((dev) => {
            const Icon = categoryIcon(dev.category);
            const ents = snapshot.manifest.entities.filter((e) => e.child_device_id === dev.id);

            return (
              <div
                key={dev.id}
                className="rounded-xl bg-[var(--bg-subtle)]/45 p-4 ring-1 ring-[var(--border-hairline)]"
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="flex items-center gap-2.5 min-w-0">
                    <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-[var(--bg-surface)] ring-1 ring-[var(--border-hairline)]">
                      <Icon className="h-4 w-4 text-[var(--text-secondary)]" />
                    </div>
                    <div className="min-w-0">
                      <div className="truncate text-[13px] font-medium">{dev.name}</div>
                      <div className="text-[11px] text-[var(--text-muted)]">
                        {dev.category.toUpperCase()}
                        {dev.manufacturer ? ` · ${dev.manufacturer}` : ''}
                      </div>
                    </div>
                  </div>
                  <span className="tabular shrink-0 rounded-md bg-[var(--bg-surface)] px-2 py-0.5 text-[10px] text-[var(--text-muted)] ring-1 ring-[var(--border-hairline)]">
                    {ents.length} entité(s)
                  </span>
                </div>

                {ents.length > 0 && (
                  <div className="hairline-t mt-3 grid grid-cols-2 gap-x-4 gap-y-1.5 pt-3">
                    {ents.map((ent) => (
                      <div key={ent.id} className="flex items-baseline justify-between gap-2 text-[11px]">
                        <span className="truncate text-[var(--text-muted)]">
                          {localizedEntityLabel(ent.name)}
                        </span>
                        <span className="tabular shrink-0 font-medium text-[var(--text-primary)]">
                          {formatEntityReading(ent)}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </SurfaceCard>

      {/* USB HID Raw Controller Table */}
      <SurfaceCard title="Inventaire USB HID natif" noPadding>
        {snapshot.hid_devices.length === 0 ? (
          <div className="px-5 pb-5 text-xs text-[var(--text-muted)]">
            Aucun contrôleur USB HID RGB/AIO détecté.
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead>
                <tr className="hairline-b text-[10px] font-medium tracking-[0.05em] text-[var(--text-muted)] uppercase">
                  <th className="px-5 py-2.5">Périphérique</th>
                  <th className="px-4 py-2.5">Fabricant</th>
                  <th className="px-4 py-2.5">VID:PID</th>
                  <th className="px-5 py-2.5 text-right">Pilote natif</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-[var(--border-hairline)]">
                {snapshot.hid_devices.map((h) => (
                  <tr key={h.path} className="transition hover:bg-black/[0.02] dark:hover:bg-white/[0.02]">
                    <td className="px-5 py-2.5 font-medium">
                      <span className="inline-flex items-center gap-2">
                        <Usb className="h-3.5 w-3.5 text-[var(--text-muted)]" />
                        {h.product}
                      </span>
                    </td>
                    <td className="px-4 py-2.5 text-[var(--text-secondary)]">{h.manufacturer}</td>
                    <td className="tabular px-4 py-2.5 text-[var(--text-muted)]">
                      {h.vendor_id.toString(16).padStart(4, '0').toUpperCase()}:
                      {h.product_id.toString(16).padStart(4, '0').toUpperCase()}
                    </td>
                    <td className="px-5 py-2.5 text-right">
                      <span className="tabular rounded-md bg-[var(--bg-subtle)] px-2 py-0.5 text-[11px] text-[var(--text-secondary)]">
                        {h.driver_hint}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </SurfaceCard>
    </div>
  );
}
