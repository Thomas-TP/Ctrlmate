import { useState } from 'react';
import {
  Bell,
  Check,
  Download,
  HardDrive,
  Headphones,
  Lock,
  MonitorOff,
  Moon,
  Package,
  Play,
  Plus,
  Power,
  RefreshCw,
  Terminal,
  Trash2,
  Volume2,
  VolumeX,
} from 'lucide-react';
import {
  checkAppUpdate,
  installAppUpdate,
  pressButton,
  sendCommand,
  updateSettings,
} from '../api';
import { StatusBadge, SurfaceCard } from '../components/Primitives';
import { useSnapshotHelpers } from '../hooks/useTelemetryHistory';
import type { AllowlistedScript, AppUpdateStatus, DaemonSnapshot } from '../types';

export function SystemView({
  snapshot,
  onSnapshot,
  onNotify,
}: {
  snapshot: DaemonSnapshot;
  onSnapshot: (s: DaemonSnapshot) => void;
  onNotify: (msg: string) => void;
}) {
  const { numVal, boolVal, textVal, updateVal } = useSnapshotHelpers(snapshot);
  const [confirmPower, setConfirmPower] = useState<'power_restart' | 'power_shutdown' | null>(null);
  const [appUpdate, setAppUpdate] = useState<AppUpdateStatus | null>(null);
  const [checkingApp, setCheckingApp] = useState(false);
  const [installingApp, setInstallingApp] = useState(false);

  const [newScriptName, setNewScriptName] = useState('');
  const [newScriptExe, setNewScriptExe] = useState('');
  const [newScriptArgs, setNewScriptArgs] = useState('');

  const muted = boolVal('audio_mute');
  const volume = numVal('master_volume') ?? 50;
  const currentAudioOut = textVal('audio_output');
  const audioOutputs =
    snapshot.manifest.entities.find((e) => e.id === 'audio_output')?.options ?? [];
  const osUpd = updateVal();
  const pendingOsCount = osUpd?.pending_count ?? 0;

  return (
    <div className="view-enter space-y-5">
      {/* Row 1: Audio Studio + Power Controls */}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-12">
        {/* Audio Studio (7 cols) */}
        <SurfaceCard title="Audio (WASAPI / PipeWire)" className="lg:col-span-7">
          <div className="space-y-4">
            <div className="flex items-center gap-3.5">
              <button
                type="button"
                onClick={() =>
                  sendCommand({
                    action: 'set_switch',
                    entity_id: 'audio_mute',
                    state: !muted,
                  })
                }
                className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-xl ring-1 transition ${
                  muted
                    ? 'bg-amber-500/10 text-amber-600 ring-amber-500/25 dark:text-amber-400'
                    : 'bg-[var(--bg-subtle)] text-[var(--text-primary)] ring-[var(--border-hairline)] hover:ring-[var(--border-strong)]'
                }`}
              >
                {muted ? <VolumeX className="h-4 w-4" /> : <Volume2 className="h-4 w-4" />}
              </button>
              <div className="flex-1 space-y-1.5">
                <div className="flex items-center justify-between text-xs">
                  <span className="text-[var(--text-secondary)]">Volume principal</span>
                  <span className="tabular font-semibold">
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

            <div className="space-y-1.5">
              <div className="text-[11px] font-medium text-[var(--text-muted)]">
                Sorties de lecture détectées (bascule 1-clic)
              </div>
              <div className="space-y-1.5">
                {audioOutputs.map((out) => {
                  const selected = out === currentAudioOut;
                  return (
                    <button
                      key={out}
                      type="button"
                      onClick={async () => {
                        const r = await sendCommand({
                          action: 'set_select',
                          entity_id: 'audio_output',
                          option: out,
                        });
                        onNotify(r.message);
                      }}
                      className={`flex w-full items-center justify-between gap-2 rounded-lg px-3 py-2 text-left text-xs transition ${
                        selected
                          ? 'bg-neutral-900 text-white dark:bg-white/[0.11] dark:text-white'
                          : 'bg-[var(--bg-subtle)]/55 text-[var(--text-secondary)] ring-1 ring-[var(--border-hairline)] hover:text-[var(--text-primary)]'
                      }`}
                    >
                      <span className="flex min-w-0 items-center gap-2">
                        <Headphones className="h-3.5 w-3.5 shrink-0 opacity-75" />
                        <span className="truncate font-medium">{out}</span>
                      </span>
                      {selected && <Check className="h-3.5 w-3.5 shrink-0" />}
                    </button>
                  );
                })}
              </div>
            </div>
          </div>
        </SurfaceCard>

        {/* Power & Session Control (5 cols) */}
        <SurfaceCard title="Alimentation & session" className="lg:col-span-5">
          <div className="space-y-3">
            <div className="grid grid-cols-2 gap-2">
              {[
                { id: 'power_lock', label: 'Verrouiller', icon: Lock },
                { id: 'display_off', label: 'Écrans off', icon: MonitorOff },
                { id: 'power_sleep', label: 'Veille', icon: Moon },
                { id: 'power_hibernate', label: 'Hibernation', icon: HardDrive },
              ].map((act) => {
                const Icon = act.icon;
                return (
                  <button
                    key={act.id}
                    type="button"
                    onClick={async () => {
                      const r = await pressButton(act.id);
                      onNotify(r.message);
                    }}
                    className="flex items-center gap-2 rounded-xl bg-[var(--bg-subtle)]/60 px-3 py-2.5 text-xs font-medium text-[var(--text-primary)] ring-1 ring-[var(--border-hairline)] transition hover:ring-[var(--border-strong)]"
                  >
                    <Icon className="h-3.5 w-3.5 text-[var(--text-muted)]" />
                    <span>{act.label}</span>
                  </button>
                );
              })}
            </div>

            <div className="hairline-t pt-3">
              <div className="mb-2 text-[10px] font-medium tracking-[0.05em] text-[var(--text-muted)] uppercase">
                Actions critiques (double confirmation)
              </div>
              <div className="grid grid-cols-2 gap-2">
                {(
                  [
                    { id: 'power_restart', label: 'Redémarrer', icon: RefreshCw },
                    { id: 'power_shutdown', label: 'Éteindre', icon: Power },
                  ] as const
                ).map((act) => {
                  const Icon = act.icon;
                  const armed = confirmPower === act.id;
                  return (
                    <button
                      key={act.id}
                      type="button"
                      onClick={async () => {
                        if (!armed) {
                          setConfirmPower(act.id);
                          setTimeout(
                            () => setConfirmPower((cur) => (cur === act.id ? null : cur)),
                            4000,
                          );
                          return;
                        }
                        setConfirmPower(null);
                        const r = await pressButton(act.id);
                        onNotify(r.message);
                      }}
                      className={`flex items-center justify-center gap-1.5 rounded-xl px-3 py-2.5 text-xs font-medium transition ${
                        armed
                          ? 'bg-red-600 text-white shadow-sm'
                          : 'bg-red-500/[0.06] text-red-600 ring-1 ring-red-500/20 hover:bg-red-500/[0.12] dark:text-red-400'
                      }`}
                    >
                      <Icon className="h-3.5 w-3.5" />
                      <span>{armed ? 'Confirmer ?' : act.label}</span>
                    </button>
                  );
                })}
              </div>
            </div>
          </div>
        </SurfaceCard>
      </div>

      {/* Row 2: Dual Update Cards (Velopack App + OS Updates) */}
      <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
        <SurfaceCard
          title="Application Ctrlmate (Velopack)"
          action={
            <StatusBadge tone={appUpdate?.update_available ? 'amber' : 'emerald'}>
              v{snapshot.manifest.host.sw_version}
            </StatusBadge>
          }
        >
          <div className="space-y-3">
            <div className="flex items-start gap-2.5">
              <Package className="mt-0.5 h-4 w-4 shrink-0 text-[var(--text-muted)]" />
              <p className="text-xs leading-relaxed text-[var(--text-secondary)]">
                {appUpdate
                  ? appUpdate.message
                  : 'Mises à jour binaires delta gérées nativement par Velopack depuis GitHub Releases.'}
              </p>
            </div>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                disabled={checkingApp}
                onClick={async () => {
                  setCheckingApp(true);
                  try {
                    const res = await checkAppUpdate();
                    setAppUpdate(res);
                    onNotify(res.message);
                  } finally {
                    setCheckingApp(false);
                  }
                }}
                className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium ring-1 ring-[var(--border-strong)] transition hover:bg-black/[0.04] disabled:opacity-50 dark:hover:bg-white/[0.06]"
              >
                <RefreshCw className={`h-3.5 w-3.5 ${checkingApp ? 'animate-spin' : ''}`} />
                Vérifier
              </button>
              {appUpdate?.update_available && (
                <button
                  type="button"
                  disabled={installingApp}
                  onClick={async () => {
                    setInstallingApp(true);
                    try {
                      onNotify(await installAppUpdate());
                    } catch (e) {
                      onNotify(String(e));
                    } finally {
                      setInstallingApp(false);
                    }
                  }}
                  className="inline-flex items-center gap-1.5 rounded-lg bg-neutral-900 px-3 py-1.5 text-xs font-medium text-white dark:bg-white dark:text-neutral-950"
                >
                  <Download className="h-3.5 w-3.5" />
                  Installer & redémarrer
                </button>
              )}
            </div>
          </div>
        </SurfaceCard>

        <SurfaceCard
          title="Système d'exploitation (WUA / PackageKit)"
          action={
            <StatusBadge
              tone={osUpd?.reboot_required ? 'amber' : pendingOsCount > 0 ? 'blue' : 'emerald'}
            >
              {osUpd?.reboot_required
                ? 'Redémarrage requis'
                : pendingOsCount > 0
                  ? `${pendingOsCount} en attente`
                  : 'À jour'}
            </StatusBadge>
          }
        >
          <div className="space-y-3">
            <div className="text-xs text-[var(--text-secondary)]">
              {osUpd?.in_progress
                ? 'Téléchargement et installation des mises à jour en cours…'
                : osUpd?.installed_version ?? snapshot.manifest.host.os_version}
            </div>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                onClick={async () => {
                  const r = await sendCommand({ action: 'check_os_updates' });
                  onNotify(r.message);
                }}
                className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium ring-1 ring-[var(--border-strong)] transition hover:bg-black/[0.04] dark:hover:bg-white/[0.06]"
              >
                <RefreshCw className="h-3.5 w-3.5" />
                Rechercher
              </button>
              {pendingOsCount > 0 && (
                <button
                  type="button"
                  disabled={osUpd?.in_progress}
                  onClick={async () => {
                    const r = await sendCommand({ action: 'install_os_updates' });
                    onNotify(r.message);
                  }}
                  className="inline-flex items-center gap-1.5 rounded-lg bg-neutral-900 px-3 py-1.5 text-xs font-medium text-white disabled:opacity-50 dark:bg-white dark:text-neutral-950"
                >
                  <Download className="h-3.5 w-3.5" />
                  Installer
                </button>
              )}
              <button
                type="button"
                onClick={async () => {
                  const r = await sendCommand({
                    action: 'send_notification',
                    title: 'Ctrlmate',
                    message: 'Les notifications bureau natives sont opérationnelles.',
                  });
                  onNotify(r.message);
                }}
                className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium ring-1 ring-[var(--border-hairline)] transition hover:bg-black/[0.04] dark:hover:bg-white/[0.06]"
              >
                <Bell className="h-3.5 w-3.5" />
                Test notif
              </button>
            </div>
            {osUpd && osUpd.pending_titles.length > 0 && (
              <ul className="hairline-t space-y-1 pt-2.5 text-[11px] text-[var(--text-muted)]">
                {osUpd.pending_titles.map((t, i) => (
                  <li key={i} className="truncate">
                    • {t}
                  </li>
                ))}
              </ul>
            )}
          </div>
        </SurfaceCard>
      </div>

      {/* Row 3: Allowlisted Custom Scripts */}
      <SurfaceCard
        title="Scripts & commandes personnalisées (liste blanche → boutons Home Assistant)"
        subtitle="Chaque commande autorisée ici apparaît automatiquement en tant que bouton natif dans Home Assistant."
      >
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-12">
            <input
              value={newScriptName}
              onChange={(e) => setNewScriptName(e.target.value)}
              placeholder="Nom du bouton"
              className="rounded-lg bg-[var(--bg-subtle)] px-3 py-2 text-xs ring-1 ring-[var(--border-hairline)] sm:col-span-3"
            />
            <input
              value={newScriptExe}
              onChange={(e) => setNewScriptExe(e.target.value)}
              placeholder="Exécutable (ex. steam.exe)"
              className="rounded-lg bg-[var(--bg-subtle)] px-3 py-2 text-xs ring-1 ring-[var(--border-hairline)] sm:col-span-4"
            />
            <input
              value={newScriptArgs}
              onChange={(e) => setNewScriptArgs(e.target.value)}
              placeholder="Arguments (optionnel)"
              className="rounded-lg bg-[var(--bg-subtle)] px-3 py-2 text-xs ring-1 ring-[var(--border-hairline)] sm:col-span-3"
            />
            <button
              type="button"
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
                onSnapshot(await updateSettings({ scripts: [...snapshot.scripts, nextItem] }));
                setNewScriptName('');
                setNewScriptExe('');
                setNewScriptArgs('');
                onNotify(`Script « ${nextItem.name} » ajouté`);
              }}
              className="inline-flex items-center justify-center gap-1.5 rounded-lg bg-neutral-900 px-3 py-2 text-xs font-medium text-white transition hover:opacity-90 sm:col-span-2 dark:bg-white dark:text-neutral-950"
            >
              <Plus className="h-3.5 w-3.5" />
              Ajouter
            </button>
          </div>

          {snapshot.scripts.length === 0 ? (
            <div className="flex items-center gap-2 rounded-xl bg-[var(--bg-subtle)]/40 px-4 py-3 text-xs text-[var(--text-muted)]">
              <Terminal className="h-4 w-4 shrink-0" />
              <span>
                Aucun script configuré. Les scripts s’exécutent strictement en session utilisateur
                standard (jamais de shell arbitraire).
              </span>
            </div>
          ) : (
            <div className="divide-y divide-[var(--border-hairline)] rounded-xl ring-1 ring-[var(--border-hairline)]">
              {snapshot.scripts.map((sc) => (
                <div
                  key={sc.id}
                  className="flex items-center justify-between gap-3 px-4 py-2.5 text-xs"
                >
                  <div className="min-w-0">
                    <div className="font-medium">{sc.name}</div>
                    <div className="tabular truncate text-[11px] text-[var(--text-muted)]">
                      {sc.executable} {sc.args.join(' ')}
                    </div>
                  </div>
                  <div className="flex shrink-0 items-center gap-1.5">
                    <button
                      type="button"
                      onClick={async () => {
                        const r = await pressButton(`script_${sc.id}`);
                        onNotify(r.message);
                      }}
                      title="Exécuter maintenant"
                      className="rounded-lg p-1.5 ring-1 ring-[var(--border-hairline)] transition hover:bg-black/[0.04] dark:hover:bg-white/[0.06]"
                    >
                      <Play className="h-3.5 w-3.5" />
                    </button>
                    <button
                      type="button"
                      onClick={async () => {
                        const next = snapshot.scripts.filter((x) => x.id !== sc.id);
                        onSnapshot(await updateSettings({ scripts: next }));
                      }}
                      title="Supprimer"
                      className="rounded-lg p-1.5 text-[var(--text-muted)] transition hover:text-red-500"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      </SurfaceCard>
    </div>
  );
}
