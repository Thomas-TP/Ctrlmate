import { useMemo, useState } from 'react';
import { Palette, RefreshCw, Sparkles, Sun } from 'lucide-react';
import { kelvinToPreviewHex, rescanHardware, setLight } from '../api';
import { StatusBadge, SurfaceCard, ToggleSwitch } from '../components/Primitives';
import { useSnapshotHelpers } from '../hooks/useTelemetryHistory';
import type { DaemonSnapshot, RgbColor } from '../types';

const KELVIN_SCENES: { name: string; kelvin: number; desc: string }[] = [
  { name: 'Bougie', kelvin: 2000, desc: 'Ambre très chaud' },
  { name: 'Détente', kelvin: 2700, desc: 'Hue Warm White' },
  { name: 'Lecture', kelvin: 3500, desc: 'Blanc doux' },
  { name: 'Neutre', kelvin: 4000, desc: 'Équilibré' },
  { name: 'Focus', kelvin: 5000, desc: 'Blanc naturel' },
  { name: 'Jour', kelvin: 6500, desc: 'Lumière du jour' },
];

const STUDIO_SWATCHES: { name: string; rgb: RgbColor }[] = [
  { name: 'Lin chaud', rgb: { r: 255, g: 206, b: 158 } },
  { name: 'Ivoire', rgb: { r: 255, g: 244, b: 232 } },
  { name: 'Ambre', rgb: { r: 242, g: 154, b: 74 } },
  { name: 'Terracotta', rgb: { r: 224, g: 108, b: 86 } },
  { name: 'Sauge', rgb: { r: 118, g: 184, b: 146 } },
  { name: 'Cèdre', rgb: { r: 92, g: 162, b: 154 } },
  { name: 'Ardoise', rgb: { r: 108, g: 154, b: 214 } },
  { name: 'Indigo doux', rgb: { r: 132, g: 136, b: 214 } },
  { name: 'Lavande', rgb: { r: 168, g: 142, b: 212 } },
  { name: 'Rose poudré', rgb: { r: 216, g: 140, b: 164 } },
];

const TRANSITION_OPTIONS: { label: string; ms: number }[] = [
  { label: 'Instantané', ms: 120 },
  { label: 'Fluide', ms: 400 },
  { label: 'Cinéma', ms: 1200 },
];

function rgbToHex(c: RgbColor): string {
  return `#${[c.r, c.g, c.b].map((n) => n.toString(16).padStart(2, '0')).join('')}`;
}

function hexToRgb(hex: string): RgbColor | null {
  const clean = hex.replace('#', '');
  if (clean.length !== 6) return null;
  const r = parseInt(clean.slice(0, 2), 16);
  const g = parseInt(clean.slice(2, 4), 16);
  const b = parseInt(clean.slice(4, 6), 16);
  if ([r, g, b].some(Number.isNaN)) return null;
  return { r, g, b };
}

export function LightingView({
  snapshot,
  onSnapshot,
  onNotify,
}: {
  snapshot: DaemonSnapshot;
  onSnapshot: (s: DaemonSnapshot) => void;
  onNotify: (msg: string) => void;
}) {
  const { lightVal } = useSnapshotHelpers(snapshot);
  const [selectedZone, setSelectedZone] = useState<string>('light_global');
  const [modeTab, setModeTab] = useState<'kelvin' | 'rgb'>('kelvin');
  const [transitionMs, setTransitionMs] = useState<number>(400);

  const lightZones = useMemo(() => {
    const labelForZone = (id: string, fallback: string): string => {
      switch (id) {
        case 'light_global':
          return 'Global PC';
        case 'light_motherboard':
          return 'Carte mère';
        case 'light_cooler':
          return 'Watercooling & Boîtier';
        case 'light_fans':
          return 'Ventilateurs';
        case 'light_ram':
          return 'Mémoire RAM';
        case 'light_gpu':
          return 'Carte graphique';
        default:
          return fallback;
      }
    };
    return snapshot.manifest.entities
      .filter((e) => e.platform === 'light')
      .map((e) => ({ ...e, displayName: labelForZone(e.id, e.name) }));
  }, [snapshot]);

  const activeLight = lightVal(selectedZone) ?? lightVal('light_global');

  if (!activeLight) {
    return (
      <SurfaceCard title="Éclairage">
        <p className="text-xs text-[var(--text-muted)]">Aucune zone lumineuse détectée.</p>
      </SurfaceCard>
    );
  }

  const activeColorHex =
    activeLight.color_mode === 'color_temp'
      ? kelvinToPreviewHex(activeLight.color_temp_kelvin)
      : rgbToHex(activeLight.rgb);

  const brightnessPct = Math.round((activeLight.brightness / 255) * 100);
  const glowOpacity = activeLight.is_on ? 0.14 + (activeLight.brightness / 255) * 0.24 : 0;

  return (
    <div className="view-enter space-y-5">
      {/* Zone Selector Strip */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-1.5 rounded-xl bg-black/[0.035] p-1 dark:bg-white/[0.04]">
          {lightZones.map((z) => {
            const zState = lightVal(z.id);
            const dotColor = !zState?.is_on
              ? '#52525b'
              : zState.color_mode === 'color_temp'
                ? kelvinToPreviewHex(zState.color_temp_kelvin)
                : rgbToHex(zState.rgb);
            const active = selectedZone === z.id;
            return (
              <button
                key={z.id}
                type="button"
                onClick={() => {
                  setSelectedZone(z.id);
                  if (zState) {
                    setModeTab(zState.color_mode === 'color_temp' ? 'kelvin' : 'rgb');
                  }
                }}
                className={`inline-flex items-center gap-2 rounded-lg px-3 py-1.5 text-xs font-medium transition ${
                  active
                    ? 'bg-white text-neutral-900 shadow-sm ring-1 ring-black/[0.06] dark:bg-white/[0.11] dark:text-white dark:ring-white/[0.1]'
                    : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)]'
                }`}
              >
                <span
                  className="h-2 w-2 rounded-full ring-1 ring-black/15"
                  style={{ backgroundColor: dotColor }}
                />
                <span>{z.displayName}</span>
              </button>
            );
          })}
        </div>

        {/* Transition duration selector */}
        <div className="flex items-center gap-1 rounded-lg bg-black/[0.035] p-0.5 text-[11px] dark:bg-white/[0.04]">
          {TRANSITION_OPTIONS.map((t) => (
            <button
              key={t.ms}
              type="button"
              onClick={() => setTransitionMs(t.ms)}
              className={`rounded-md px-2.5 py-1 font-medium transition ${
                transitionMs === t.ms
                  ? 'bg-white text-neutral-900 shadow-xs dark:bg-white/[0.1] dark:text-white'
                  : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>
      </div>

      {/* Main Architectural Studio Stage */}
      <div className="surface-card relative overflow-hidden rounded-2xl p-6">
        {/* Calibrated studio lamp radial glow (subtle, never harsh neon) */}
        <div
          className="pointer-events-none absolute -top-24 -right-24 h-72 w-72 rounded-full blur-3xl transition-all duration-500"
          style={{
            backgroundColor: activeColorHex,
            opacity: glowOpacity,
          }}
        />

        <div className="relative space-y-6">
          {/* Header with live lamp preview + switch */}
          <div className="flex flex-wrap items-center justify-between gap-4">
            <div className="flex items-center gap-4">
              <div
                className="flex h-14 w-14 items-center justify-center rounded-2xl ring-1 ring-black/10 transition-all duration-300 dark:ring-white/15"
                style={{
                  backgroundColor: activeLight.is_on ? activeColorHex : '#1f2026',
                  boxShadow: activeLight.is_on
                    ? `0 10px 28px -6px ${activeColorHex}88`
                    : undefined,
                }}
              >
                <Sun
                  className={`h-6 w-6 transition ${
                    activeLight.is_on ? 'text-neutral-900/75' : 'text-neutral-500'
                  }`}
                />
              </div>
              <div>
                <div className="flex items-baseline gap-2.5">
                  <span className="tabular text-2xl font-semibold tracking-[-0.03em]">
                    {activeLight.is_on
                      ? activeLight.color_mode === 'color_temp'
                        ? `${activeLight.color_temp_kelvin} K`
                        : activeColorHex.toUpperCase()
                      : 'Éteint'}
                  </span>
                  {activeLight.is_on && (
                    <span className="tabular text-sm font-medium text-[var(--text-muted)]">
                      {brightnessPct} %
                    </span>
                  )}
                </div>
                <div className="mt-0.5 text-xs text-[var(--text-muted)]">
                  {activeLight.color_mode === 'color_temp'
                    ? 'Spectre blanc calibré Philips Hue / Govee'
                    : `Couleur directe · RGB(${activeLight.rgb.r}, ${activeLight.rgb.g}, ${activeLight.rgb.b})`}
                </div>
              </div>
            </div>

            <div className="flex items-center gap-3">
              {/* Contextual Mode Switch (White Spectrum vs Direct Color) */}
              <div className="flex rounded-lg bg-black/[0.04] p-0.5 text-xs dark:bg-white/[0.05]">
                <button
                  type="button"
                  onClick={() => setModeTab('kelvin')}
                  className={`inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 font-medium transition ${
                    modeTab === 'kelvin'
                      ? 'bg-white text-neutral-900 shadow-xs dark:bg-white/[0.12] dark:text-white'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                  }`}
                >
                  <Sparkles className="h-3.5 w-3.5" />
                  Chaleur Kelvin
                </button>
                <button
                  type="button"
                  onClick={() => setModeTab('rgb')}
                  className={`inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 font-medium transition ${
                    modeTab === 'rgb'
                      ? 'bg-white text-neutral-900 shadow-xs dark:bg-white/[0.12] dark:text-white'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                  }`}
                >
                  <Palette className="h-3.5 w-3.5" />
                  Couleur RGB
                </button>
              </div>

              <ToggleSwitch
                checked={activeLight.is_on}
                label="Allumer ou éteindre l'éclairage"
                onChange={(next) =>
                  setLight({
                    entity_id: selectedZone,
                    is_on: next,
                    transition_ms: transitionMs,
                  })
                }
              />
            </div>
          </div>

          {/* Master Intensity Control */}
          <div className="space-y-2">
            <div className="flex items-center justify-between text-xs">
              <span className="font-medium text-[var(--text-secondary)]">Intensité lumineuse</span>
              <span className="tabular font-medium">{brightnessPct} %</span>
            </div>
            <input
              type="range"
              min={0}
              max={255}
              value={activeLight.brightness}
              onChange={(e) =>
                setLight({
                  entity_id: selectedZone,
                  is_on: Number(e.target.value) > 0,
                  brightness: Number(e.target.value),
                  transition_ms: 120,
                })
              }
              className="ctrl-slider w-full"
              style={{
                background: `linear-gradient(90deg, rgba(82,82,91,0.28) 0%, ${activeColorHex} 100%)`,
                borderRadius: '9999px',
              }}
            />
          </div>

          {/* Contextual Panel: White Kelvin Spectrum OR Direct RGB Color */}
          {modeTab === 'kelvin' ? (
            <div className="hairline-t space-y-4 pt-5">
              <div className="space-y-2">
                <div className="flex items-center justify-between text-xs">
                  <span className="font-medium text-[var(--text-secondary)]">
                    Température de couleur (2000K – 6500K)
                  </span>
                  <span className="tabular font-medium">{activeLight.color_temp_kelvin} K</span>
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
                      is_on: true,
                      color_temp_kelvin: Number(e.target.value),
                      transition_ms: 160,
                    })
                  }
                  className="ctrl-slider w-full"
                  style={{
                    background:
                      'linear-gradient(90deg, #ff9638 0%, #ffc88a 30%, #fff0dc 62%, #ebf2ff 100%)',
                    borderRadius: '9999px',
                  }}
                />
                <div className="tabular flex justify-between text-[10px] text-[var(--text-muted)]">
                  <span>2000K · Chaud</span>
                  <span>3500K · Doux</span>
                  <span>5000K · Naturel</span>
                  <span>6500K · Jour</span>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-6">
                {KELVIN_SCENES.map((scene) => {
                  const selected =
                    activeLight.is_on &&
                    activeLight.color_mode === 'color_temp' &&
                    Math.abs(activeLight.color_temp_kelvin - scene.kelvin) <= 150;
                  return (
                    <button
                      key={scene.kelvin}
                      type="button"
                      onClick={() =>
                        setLight({
                          entity_id: selectedZone,
                          is_on: true,
                          color_temp_kelvin: scene.kelvin,
                          transition_ms: transitionMs,
                        })
                      }
                      className={`rounded-xl p-2.5 text-left transition ${
                        selected
                          ? 'bg-neutral-900 text-white dark:bg-white dark:text-neutral-950'
                          : 'bg-[var(--bg-subtle)]/60 ring-1 ring-[var(--border-hairline)] hover:ring-[var(--border-strong)]'
                      }`}
                    >
                      <div className="flex items-center justify-between">
                        <span
                          className="h-3.5 w-3.5 rounded-full ring-1 ring-black/15"
                          style={{ backgroundColor: kelvinToPreviewHex(scene.kelvin) }}
                        />
                        <span
                          className={`tabular text-[10px] ${
                            selected
                              ? 'text-white/75 dark:text-neutral-600'
                              : 'text-[var(--text-muted)]'
                          }`}
                        >
                          {scene.kelvin}K
                        </span>
                      </div>
                      <div className="mt-2 text-xs font-medium">{scene.name}</div>
                      <div
                        className={`text-[10px] ${
                          selected
                            ? 'text-white/70 dark:text-neutral-600'
                            : 'text-[var(--text-muted)]'
                        }`}
                      >
                        {scene.desc}
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>
          ) : (
            <div className="hairline-t space-y-4 pt-5">
              <div>
                <div className="mb-2.5 text-xs font-medium text-[var(--text-secondary)]">
                  Palette architecturale (teintes équilibrées)
                </div>
                <div className="grid grid-cols-5 gap-2 sm:grid-cols-10">
                  {STUDIO_SWATCHES.map((sw) => {
                    const hex = rgbToHex(sw.rgb);
                    return (
                      <button
                        key={sw.name}
                        type="button"
                        title={sw.name}
                        onClick={() =>
                          setLight({
                            entity_id: selectedZone,
                            is_on: true,
                            rgb: sw.rgb,
                            transition_ms: transitionMs,
                          })
                        }
                        className="group flex flex-col items-center gap-1.5 rounded-lg p-1.5 transition hover:bg-black/[0.04] dark:hover:bg-white/[0.05]"
                      >
                        <span
                          className="h-7 w-7 rounded-full ring-1 ring-black/15 transition group-hover:scale-110 dark:ring-white/15"
                          style={{ backgroundColor: hex }}
                        />
                        <span className="w-full truncate text-center text-[10px] text-[var(--text-muted)]">
                          {sw.name.split(' ')[0]}
                        </span>
                      </button>
                    );
                  })}
                </div>
              </div>

              <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl bg-[var(--bg-subtle)]/55 px-4 py-3 ring-1 ring-[var(--border-hairline)]">
                <div className="flex items-center gap-3">
                  <label className="relative flex cursor-pointer items-center gap-2 text-xs font-medium">
                    <input
                      type="color"
                      value={rgbToHex(activeLight.rgb)}
                      onChange={(e) => {
                        const parsed = hexToRgb(e.target.value);
                        if (parsed) {
                          setLight({
                            entity_id: selectedZone,
                            is_on: true,
                            rgb: parsed,
                            transition_ms: 150,
                          });
                        }
                      }}
                      className="h-8 w-10 cursor-pointer rounded-md border-0 bg-transparent p-0"
                    />
                    <span>Sélecteur libre</span>
                  </label>
                </div>
                <div className="tabular flex items-center gap-3 text-xs text-[var(--text-secondary)]">
                  <span>R {activeLight.rgb.r}</span>
                  <span>G {activeLight.rgb.g}</span>
                  <span>B {activeLight.rgb.b}</span>
                  <span className="rounded bg-[var(--bg-surface)] px-2 py-0.5 font-medium text-[var(--text-primary)] ring-1 ring-[var(--border-hairline)]">
                    {rgbToHex(activeLight.rgb).toUpperCase()}
                  </span>
                </div>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Physical Controllers & Hardware Diagnostics */}
      <SurfaceCard
        title="Contrôleurs physiques & diagnostics RGB"
        subtitle="Gigabyte IT5702 · NZXT H9 Flow · G.Skill DDR5 ENE SMBus · Samsung 990 PRO Heatsink"
        action={
          <button
            type="button"
            onClick={async () => {
              onSnapshot(await rescanHardware());
              onNotify('Contrôleurs RGB rescannés');
            }}
            className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium text-[var(--text-secondary)] ring-1 ring-black/[0.08] transition hover:bg-black/[0.04] hover:text-[var(--text-primary)] dark:ring-white/[0.1] dark:hover:bg-white/[0.06]"
          >
            <RefreshCw className="h-3 w-3" />
            Rescanner
          </button>
        }
        noPadding
      >
        <div className="divide-y divide-[var(--border-hairline)]">
          {(snapshot.rgb_devices ?? []).length === 0 ? (
            <div className="px-5 pb-5 text-xs text-[var(--text-muted)]">
              Aucun contrôleur RGB détecté. Fermez les utilitaires constructeur puis cliquez
              Rescanner.
            </div>
          ) : (
            snapshot.rgb_devices.map((dev) => (
              <div
                key={dev.id}
                className="flex flex-col justify-between gap-3 px-5 py-3.5 sm:flex-row sm:items-start"
              >
                <div className="space-y-1">
                  <div className="flex flex-wrap items-center gap-2 text-xs font-medium">
                    <span>{dev.name}</span>
                    <span className="tabular rounded bg-[var(--bg-subtle)] px-1.5 py-0.5 text-[10px] font-normal text-[var(--text-muted)]">
                      {dev.transport}
                    </span>
                  </div>
                  <p className="text-[11px] leading-relaxed text-[var(--text-muted)]">
                    {dev.reason}
                  </p>
                </div>
                <div className="shrink-0">
                  <StatusBadge tone={dev.controllable ? 'emerald' : 'neutral'}>
                    {dev.controllable ? 'Contrôle actif' : 'Diagnostic / Opt-in'}
                  </StatusBadge>
                </div>
              </div>
            ))
          )}
        </div>
      </SurfaceCard>
    </div>
  );
}
