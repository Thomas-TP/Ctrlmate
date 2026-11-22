import { Droplets, Fan, Flame, ShieldAlert, ShieldCheck, Wind } from 'lucide-react';
import { formatTemp, setCoolingPreset } from '../api';
import { StatusBadge, SurfaceCard } from '../components/Primitives';
import { useSnapshotHelpers } from '../hooks/useTelemetryHistory';
import type { CoolingPreset, DaemonSnapshot, TemperatureUnit } from '../types';

interface PresetSpec {
  id: CoolingPreset;
  label: string;
  desc: string;
  acousticLevel: 1 | 2 | 3 | 4 | 5;
  pumpFloor: number;
  fanCurve: [number, number][]; // [tempC, duty%]
  pumpCurve: [number, number][];
}

const PRESET_SPECS: PresetSpec[] = [
  {
    id: 'auto',
    label: 'Auto',
    desc: 'Courbe matérielle / BIOS native sans forçage logiciel.',
    acousticLevel: 2,
    pumpFloor: 65,
    fanCurve: [
      [25, 30],
      [40, 38],
      [55, 52],
      [70, 72],
      [85, 100],
    ],
    pumpCurve: [
      [25, 65],
      [45, 72],
      [65, 85],
      [85, 100],
    ],
  },
  {
    id: 'silent',
    label: 'Silencieux',
    desc: 'Acoustique minimale au repos, montée douce sous charge.',
    acousticLevel: 1,
    pumpFloor: 55,
    fanCurve: [
      [25, 22],
      [45, 28],
      [62, 42],
      [76, 65],
      [86, 100],
    ],
    pumpCurve: [
      [25, 55],
      [50, 62],
      [72, 78],
      [86, 100],
    ],
  },
  {
    id: 'balanced',
    label: 'Équilibré',
    desc: 'Compromis idéal entre température et discrétion.',
    acousticLevel: 3,
    pumpFloor: 70,
    fanCurve: [
      [25, 35],
      [45, 48],
      [62, 65],
      [76, 85],
      [85, 100],
    ],
    pumpCurve: [
      [25, 70],
      [50, 78],
      [70, 90],
      [85, 100],
    ],
  },
  {
    id: 'performance',
    label: 'Performance',
    desc: 'Flux d’air soutenu pour sessions gaming / rendu prolongées.',
    acousticLevel: 4,
    pumpFloor: 85,
    fanCurve: [
      [25, 55],
      [45, 70],
      [62, 85],
      [75, 96],
      [85, 100],
    ],
    pumpCurve: [
      [25, 85],
      [55, 92],
      [75, 100],
      [85, 100],
    ],
  },
  {
    id: 'full_speed',
    label: 'Maximum',
    desc: 'Refroidissement 100 % immédiat (purge thermique).',
    acousticLevel: 5,
    pumpFloor: 100,
    fanCurve: [
      [25, 100],
      [85, 100],
    ],
    pumpCurve: [
      [25, 100],
      [85, 100],
    ],
  },
];

export function CoolingView({
  snapshot,
  unit,
  onNotify,
}: {
  snapshot: DaemonSnapshot;
  unit: TemperatureUnit;
  onNotify: (msg: string) => void;
}) {
  const { numVal, textVal } = useSnapshotHelpers(snapshot);
  const activePreset = (textVal('cooling_preset') || 'auto') as CoolingPreset;
  const spec = PRESET_SPECS.find((p) => p.id === activePreset) ?? PRESET_SPECS[0];

  const liquidTemp = numVal('cooler_0_liquid_temperature');
  const pumpRpm = numVal('cooler_0_pump_rpm');
  const pumpDuty = numVal('cooler_0_pump_duty');
  const fanRpm = numVal('cooler_0_fan_rpm');
  const fanDuty = numVal('cooler_0_fan_duty');
  const gpuTemp = numVal('gpu_0_temperature');
  const gpuFan = numVal('gpu_0_fan_speed');
  const cpuTemp = numVal('cpu_0_temperature');

  // Operating point plotted on curve (prefers GPU/CPU package temp, falls back to liquid+18°C heuristic)
  const plotTemp = cpuTemp ?? gpuTemp ?? (liquidTemp !== undefined ? liquidTemp + 15 : 42);
  const plotDuty = fanDuty ?? gpuFan ?? 40;

  return (
    <div className="view-enter space-y-5">
      {/* Live Thermal Instrumentation Bar */}
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <ThermalMetric
          icon={<Droplets className="h-3.5 w-3.5" />}
          label="Liquide AIO"
          value={formatTemp(liquidTemp, unit)}
          sub={`Seuil failsafe ${formatTemp(48, unit)}`}
        />
        <ThermalMetric
          icon={<Wind className="h-3.5 w-3.5" />}
          label="Pompe Watercooling"
          value={pumpRpm !== undefined ? `${pumpRpm} RPM` : '—'}
          sub={pumpDuty !== undefined ? `Duty ${pumpDuty} % · Plancher ≥ 50 %` : 'Plancher ≥ 50 %'}
        />
        <ThermalMetric
          icon={<Fan className="h-3.5 w-3.5" />}
          label="Ventilateurs Radiateur"
          value={fanRpm !== undefined ? `${fanRpm} RPM` : '—'}
          sub={fanDuty !== undefined ? `Duty ${fanDuty} %` : 'NZXT Kraken HID'}
        />
        <ThermalMetric
          icon={<Flame className="h-3.5 w-3.5" />}
          label="GPU Thermique"
          value={formatTemp(gpuTemp, unit)}
          sub={gpuFan !== undefined ? `Ventilateurs ${gpuFan} %` : 'NVML'}
        />
      </div>

      {/* Architectural SVG Thermal Curve Visualization */}
      <SurfaceCard
        title={`Profil thermique actif — ${spec.label}`}
        subtitle="Courbes PWM cibles en fonction de la température · Point orange = fonctionnement temps réel"
        action={
          <StatusBadge
            tone={snapshot.thermal_failsafe_active ? 'red' : 'emerald'}
            pulse={snapshot.thermal_failsafe_active}
          >
            {snapshot.thermal_failsafe_active ? 'Failsafe 100 %' : 'Watchdog Nominal'}
          </StatusBadge>
        }
      >
        <ThermalCurveGraph
          fanCurve={spec.fanCurve}
          pumpCurve={spec.pumpCurve}
          liveTempC={plotTemp}
          liveDutyPct={plotDuty}
          unit={unit}
        />
        <div className="mt-3 flex flex-wrap items-center justify-between gap-3 text-[11px] text-[var(--text-muted)]">
          <div className="flex flex-wrap items-center gap-4">
            <span className="inline-flex items-center gap-1.5">
              <span className="h-0.5 w-4 rounded bg-neutral-900 dark:bg-white" />
              Ventilateurs PWM
            </span>
            <span className="inline-flex items-center gap-1.5">
              <span className="h-0.5 w-4 rounded border-t border-dashed border-emerald-500" />
              Pompe AIO (plancher 50 %)
            </span>
            <span className="inline-flex items-center gap-1.5">
              <span className="h-2 w-2 rounded-full bg-amber-500" />
              Point actuel ({formatTemp(plotTemp, unit)} · {plotDuty} %)
            </span>
          </div>
          <span className="tabular">
            Failsafe → 100 % à {formatTemp(83, unit)} GPU / {formatTemp(88, unit)} CPU /{' '}
            {formatTemp(48, unit)} liquide
          </span>
        </div>
      </SurfaceCard>

      {/* Preset Selector Grid */}
      <SurfaceCard title="Modes de refroidissement">
        <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-5">
          {PRESET_SPECS.map((p) => {
            const active = activePreset === p.id;
            return (
              <button
                key={p.id}
                type="button"
                onClick={async () => {
                  const r = await setCoolingPreset(p.id);
                  onNotify(r.message);
                }}
                className={`flex flex-col justify-between rounded-xl p-3.5 text-left transition ${
                  active
                    ? 'bg-neutral-900 text-white shadow-sm dark:bg-white dark:text-neutral-950'
                    : 'bg-[var(--bg-subtle)]/55 text-[var(--text-primary)] ring-1 ring-[var(--border-hairline)] hover:ring-[var(--border-strong)]'
                }`}
              >
                <div>
                  <div className="flex items-center justify-between">
                    <span className="text-[13px] font-semibold">{p.label}</span>
                    {/* Subtle acoustic intensity bars */}
                    <span className="flex items-end gap-[2px]" aria-hidden="true">
                      {[1, 2, 3, 4, 5].map((bar) => (
                        <span
                          key={bar}
                          className={`w-[2.5px] rounded-full ${
                            bar <= p.acousticLevel
                              ? active
                                ? 'bg-white dark:bg-neutral-950'
                                : 'bg-neutral-700 dark:bg-neutral-300'
                              : active
                                ? 'bg-white/25 dark:bg-neutral-950/20'
                                : 'bg-black/10 dark:bg-white/10'
                          }`}
                          style={{ height: `${5 + bar * 2}px` }}
                        />
                      ))}
                    </span>
                  </div>
                  <p
                    className={`mt-1.5 text-[11px] leading-relaxed ${
                      active
                        ? 'text-white/75 dark:text-neutral-600'
                        : 'text-[var(--text-muted)]'
                    }`}
                  >
                    {p.desc}
                  </p>
                </div>
                <div
                  className={`tabular mt-3 text-[10px] font-medium ${
                    active
                      ? 'text-white/80 dark:text-neutral-700'
                      : 'text-[var(--text-secondary)]'
                  }`}
                >
                  Pompe ≥ {p.pumpFloor} %
                </div>
              </button>
            );
          })}
        </div>
      </SurfaceCard>

      {/* Non-Bypassable Watchdog Architecture Card */}
      <SurfaceCard title="Garde-fou thermique matériel (non contournable)">
        <div className="flex items-start gap-3.5 text-xs">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-emerald-500/10 text-emerald-600 dark:text-emerald-400">
            {snapshot.thermal_failsafe_active ? (
              <ShieldAlert className="h-4 w-4 text-red-500" />
            ) : (
              <ShieldCheck className="h-4 w-4" />
            )}
          </div>
          <p className="leading-relaxed text-[var(--text-secondary)]">
            Le plancher de la pompe AIO est verrouillé en dur à{' '}
            <span className="tabular font-medium text-[var(--text-primary)]">50 % minimum</span>{' '}
            côté daemon Rust et côté contrôleur NZXT Kraken (courbe matérielle de secours à{' '}
            {formatTemp(55, unit)} liquide). En cas de dépassement critique (CPU ≥{' '}
            {formatTemp(88, unit)}, GPU ≥ {formatTemp(83, unit)} ou liquide ≥ {formatTemp(48, unit)}
            ), Ctrlmate bascule immédiatement tous les canaux à 100 % avec hystérésis et déclenche le
            capteur <code className="tabular">thermal_failsafe</code> dans Home Assistant. À
            l’arrêt du service, les courbes BIOS/NVML natives sont automatiquement restaurées.
          </p>
        </div>
      </SurfaceCard>
    </div>
  );
}

function ThermalMetric({
  icon,
  label,
  value,
  sub,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  sub: string;
}) {
  return (
    <div className="surface-card rounded-xl p-4">
      <div className="flex items-center gap-1.5 text-[11px] font-medium text-[var(--text-muted)]">
        {icon}
        <span>{label}</span>
      </div>
      <div className="tabular mt-2 text-xl font-semibold tracking-[-0.025em]">{value}</div>
      <div className="tabular mt-1 text-[11px] text-[var(--text-muted)]">{sub}</div>
    </div>
  );
}

function ThermalCurveGraph({
  fanCurve,
  pumpCurve,
  liveTempC,
  liveDutyPct,
  unit,
}: {
  fanCurve: [number, number][];
  pumpCurve: [number, number][];
  liveTempC: number;
  liveDutyPct: number;
  unit: TemperatureUnit;
}) {
  const W = 680;
  const H = 200;
  const padL = 40;
  const padR = 16;
  const padT = 14;
  const padB = 26;
  const plotW = W - padL - padR;
  const plotH = H - padT - padB;

  const mapX = (temp: number) => {
    const clamped = Math.max(20, Math.min(90, temp));
    return padL + ((clamped - 20) / 70) * plotW;
  };
  const mapY = (duty: number) => {
    const clamped = Math.max(0, Math.min(100, duty));
    return padT + ((100 - clamped) / 100) * plotH;
  };

  const buildPath = (pts: [number, number][]) => {
    const mapped = pts.map(([t, d]) => [mapX(t), mapY(d)] as const);
    if (mapped.length === 0) return '';
    let d = `M ${mapped[0][0].toFixed(1)} ${mapped[0][1].toFixed(1)}`;
    for (let i = 1; i < mapped.length; i++) {
      const [x0, y0] = mapped[i - 1];
      const [x1, y1] = mapped[i];
      const cx = ((x0 + x1) / 2).toFixed(1);
      d += ` C ${cx} ${y0.toFixed(1)}, ${cx} ${y1.toFixed(1)}, ${x1.toFixed(1)} ${y1.toFixed(1)}`;
    }
    return d;
  };

  const fanLine = buildPath(fanCurve);
  const fanArea = `${fanLine} L ${mapX(fanCurve[fanCurve.length - 1][0]).toFixed(1)} ${mapY(0).toFixed(1)} L ${mapX(fanCurve[0][0]).toFixed(1)} ${mapY(0).toFixed(1)} Z`;
  const pumpLine = buildPath(pumpCurve);
  const floorY = mapY(50);
  const failsafeX = mapX(83);
  const liveX = mapX(liveTempC);
  const liveY = mapY(liveDutyPct);

  return (
    <div className="w-full overflow-hidden rounded-lg bg-[var(--bg-subtle)]/40 p-2 ring-1 ring-[var(--border-hairline)]">
      <svg viewBox={`0 0 ${W} ${H}`} className="h-48 w-full select-none">
        <defs>
          <linearGradient id="fanFill" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="currentColor" stopOpacity="0.14" />
            <stop offset="100%" stopColor="currentColor" stopOpacity="0.01" />
          </linearGradient>
        </defs>

        {/* Pump hard floor safe zone (<50%) */}
        <rect
          x={padL}
          y={floorY}
          width={plotW}
          height={mapY(0) - floorY}
          fill="rgba(16, 185, 129, 0.045)"
        />
        <line
          x1={padL}
          y1={floorY}
          x2={W - padR}
          y2={floorY}
          stroke="rgba(16, 185, 129, 0.35)"
          strokeDasharray="3 3"
          strokeWidth="1"
        />

        {/* Horizontal Grid */}
        {[0, 25, 50, 75, 100].map((pct) => {
          const y = mapY(pct);
          return (
            <g key={pct}>
              <line
                x1={padL}
                y1={y}
                x2={W - padR}
                y2={y}
                stroke="var(--border-hairline)"
                strokeDasharray={pct === 0 ? undefined : '3 4'}
                strokeWidth="1"
              />
              <text
                x={padL - 8}
                y={y + 3}
                textAnchor="end"
                fontSize="10"
                className="tabular fill-[var(--text-muted)]"
              >
                {pct}%
              </text>
            </g>
          );
        })}

        {/* Vertical X-Axis Temperature Ticks */}
        {[20, 35, 50, 65, 80, 90].map((t) => {
          const x = mapX(t);
          const lbl = unit === 'fahrenheit' ? `${Math.round((t * 9) / 5 + 32)}°F` : `${t}°C`;
          return (
            <g key={t}>
              <line
                x1={x}
                y1={padT}
                x2={x}
                y2={mapY(0)}
                stroke="var(--border-hairline)"
                strokeDasharray="2 5"
                strokeWidth="1"
              />
              <text
                x={x}
                y={H - 6}
                textAnchor="middle"
                fontSize="10"
                className="tabular fill-[var(--text-muted)]"
              >
                {lbl}
              </text>
            </g>
          );
        })}

        {/* Failsafe Threshold Reference */}
        <line
          x1={failsafeX}
          y1={padT}
          x2={failsafeX}
          y2={mapY(0)}
          stroke="rgba(217, 119, 6, 0.45)"
          strokeDasharray="3 3"
          strokeWidth="1"
        />

        {/* Fan Area + Smooth Curve */}
        <path d={fanArea} fill="url(#fanFill)" className="text-neutral-900 dark:text-white" />
        <path
          d={fanLine}
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          className="text-neutral-900 dark:text-white"
        />

        {/* Pump Curve (Dashed Emerald) */}
        <path
          d={pumpLine}
          fill="none"
          stroke="#10b981"
          strokeWidth="1.6"
          strokeDasharray="5 4"
          strokeLinecap="round"
        />

        {/* Control Points */}
        {fanCurve.map(([t, d], idx) => (
          <circle
            key={idx}
            cx={mapX(t)}
            cy={mapY(d)}
            r="3"
            fill="var(--bg-surface)"
            stroke="currentColor"
            strokeWidth="1.8"
            className="text-neutral-900 dark:text-white"
          />
        ))}

        {/* Live Operating Point */}
        <line
          x1={liveX}
          y1={liveY}
          x2={liveX}
          y2={mapY(0)}
          stroke="rgba(245, 158, 11, 0.35)"
          strokeDasharray="2 2"
        />
        <circle cx={liveX} cy={liveY} r="7" fill="rgba(245, 158, 11, 0.2)" />
        <circle cx={liveX} cy={liveY} r="3.6" fill="#f59e0b" stroke="var(--bg-surface)" strokeWidth="1.5" />
      </svg>
    </div>
  );
}
