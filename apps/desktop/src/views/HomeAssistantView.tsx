import { useEffect, useState } from 'react';
import { Check, Copy, KeyRound, Radio, Shield, Trash2 } from 'lucide-react';
import { cancelPairing, revokeClient, startPairing, updateSettings } from '../api';
import { StatusBadge, SurfaceCard, ToggleSwitch } from '../components/Primitives';
import type {
  DaemonSnapshot,
  MqttConfig,
  PollingProfile,
  RemotePermissions,
} from '../types';

const POLLING_CARDS: { id: PollingProfile; label: string; cadence: string; desc: string }[] = [
  {
    id: 'intelligent',
    label: 'Intelligent',
    cadence: '2 s jeu · 5 s actif',
    desc: 'Deadband adaptatif pour préserver Recorder.',
  },
  {
    id: 'balanced',
    label: 'Équilibré',
    cadence: '5 s fixe',
    desc: 'Filtrage léger et réactivité constante.',
  },
  {
    id: 'eco',
    label: 'Éco',
    cadence: '10–30 s',
    desc: 'Trafic réseau et base HA minimalistes.',
  },
  {
    id: 'realtime',
    label: 'Temps réel',
    cadence: '1–2 s',
    desc: 'Flux haute fréquence pour dashboards live.',
  },
];

const PERMISSION_ROWS: { key: keyof RemotePermissions; label: string; desc: string }[] = [
  { key: 'telemetry', label: 'Télémétrie matériel', desc: 'Températures, charges, RPM, stockage' },
  { key: 'lighting', label: 'Éclairage RGB & Kelvin', desc: 'Sync Hue / Govee & zones PC' },
  { key: 'cooling', label: 'Refroidissement', desc: 'Changement de preset (Auto → Max)' },
  { key: 'audio_and_media', label: 'Audio & volume', desc: 'Volume, muet et changement de sortie' },
  { key: 'power_control', label: 'Alimentation', desc: 'Veille, verrouillage, redémarrage, arrêt' },
  { key: 'os_updates', label: 'Mises à jour OS', desc: 'Recherche et installation WUA / PackageKit' },
  { key: 'notifications', label: 'Notifications bureau', desc: 'Envoi de toasts depuis Home Assistant' },
  { key: 'custom_scripts', label: 'Scripts personnalisés', desc: 'Exécution des boutons allowlistés' },
];

export function HomeAssistantView({
  snapshot,
  onSnapshot,
  onNotify,
}: {
  snapshot: DaemonSnapshot;
  onSnapshot: (s: DaemonSnapshot) => void;
  onNotify: (msg: string) => void;
}) {
  const [copiedFp, setCopiedFp] = useState(false);
  const [mqttDraft, setMqttDraft] = useState<MqttConfig>(snapshot.mqtt);

  useEffect(() => {
    setMqttDraft(snapshot.mqtt);
  }, [snapshot.mqtt.enabled, snapshot.mqtt.host, snapshot.mqtt.port]);

  const pinDigits = snapshot.pairing.pin ? snapshot.pairing.pin.split('') : [];

  return (
    <div className="view-enter space-y-5">
      {/* Pairing Studio Card */}
      <SurfaceCard
        title="Appairage Home Assistant 2026.11 (TLS 1.3 TOFU · mDNS)"
        action={
          snapshot.pairing.active ? (
            <button
              type="button"
              onClick={async () => onSnapshot(await cancelPairing())}
              className="rounded-lg px-3 py-1.5 text-xs font-medium ring-1 ring-[var(--border-strong)] transition hover:bg-black/[0.04] dark:hover:bg-white/[0.06]"
            >
              Annuler la session
            </button>
          ) : (
            <button
              type="button"
              onClick={async () => onSnapshot(await startPairing())}
              className="inline-flex items-center gap-1.5 rounded-lg bg-neutral-900 px-3.5 py-1.5 text-xs font-medium text-white transition hover:opacity-90 dark:bg-white dark:text-neutral-950"
            >
              <KeyRound className="h-3.5 w-3.5" />
              Générer un code PIN
            </button>
          )
        }
      >
        <div className="space-y-4">
          <div className="flex flex-wrap items-center gap-3 text-xs text-[var(--text-secondary)]">
            <span className="tabular rounded-md bg-[var(--bg-subtle)] px-2.5 py-1 ring-1 ring-[var(--border-hairline)]">
              _ctrlmate._tcp · Port {snapshot.pairing.listen_port}
            </span>
            <button
              type="button"
              onClick={() => {
                navigator.clipboard?.writeText(snapshot.pairing.cert_fingerprint).catch(() => {});
                setCopiedFp(true);
                setTimeout(() => setCopiedFp(false), 1800);
                onNotify('Empreinte TLS SHA-256 copiée');
              }}
              className="tabular inline-flex items-center gap-1.5 rounded-md bg-[var(--bg-subtle)] px-2.5 py-1 text-[11px] ring-1 ring-[var(--border-hairline)] transition hover:ring-[var(--border-strong)]"
            >
              <Shield className="h-3 w-3 text-emerald-500" />
              <span>SHA-256 {snapshot.pairing.cert_fingerprint.slice(0, 18)}…</span>
              {copiedFp ? <Check className="h-3 w-3 text-emerald-500" /> : <Copy className="h-3 w-3 opacity-60" />}
            </button>
          </div>

          {snapshot.pairing.active && pinDigits.length > 0 && (
            <div className="flex flex-col items-start gap-2 rounded-xl bg-[var(--bg-subtle)]/70 p-4 ring-1 ring-[var(--border-strong)] sm:flex-row sm:items-center sm:justify-between">
              <div>
                <div className="text-xs font-medium">Code PIN d’appairage actif</div>
                <div className="text-[11px] text-[var(--text-muted)]">
                  Saisissez ce code dans Home Assistant → Intégrations → Ctrlmate.
                </div>
              </div>
              <div className="flex items-center gap-1.5">
                {pinDigits.map((d, i) => (
                  <span
                    key={i}
                    className="tabular flex h-11 w-9 items-center justify-center rounded-lg bg-[var(--bg-surface)] text-xl font-semibold shadow-xs ring-1 ring-[var(--border-strong)]"
                  >
                    {d}
                  </span>
                ))}
              </div>
            </div>
          )}

          {/* Paired instances */}
          {snapshot.paired_clients.length > 0 && (
            <div className="hairline-t space-y-2 pt-4">
              <div className="text-[11px] font-medium text-[var(--text-muted)]">
                Instances Home Assistant appairées ({snapshot.paired_clients.length})
              </div>
              <div className="divide-y divide-[var(--border-hairline)] rounded-xl ring-1 ring-[var(--border-hairline)]">
                {snapshot.paired_clients.map((cl) => (
                  <div
                    key={cl.client_id}
                    className="flex items-center justify-between px-4 py-2.5 text-xs"
                  >
                    <div className="flex items-center gap-3">
                      <StatusBadge tone={cl.connected ? 'emerald' : 'neutral'} pulse={cl.connected}>
                        {cl.connected ? 'Connecté' : 'Hors ligne'}
                      </StatusBadge>
                      <span className="font-medium">{cl.client_name}</span>
                    </div>
                    <button
                      type="button"
                      onClick={async () => {
                        onSnapshot(await revokeClient(cl.client_id));
                        onNotify(`Accès révoqué pour ${cl.client_name}`);
                      }}
                      className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] text-[var(--text-muted)] transition hover:bg-red-500/10 hover:text-red-500"
                    >
                      <Trash2 className="h-3 w-3" />
                      Révoquer
                    </button>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      </SurfaceCard>

      {/* Adaptive Polling Cadence */}
      <SurfaceCard title="Profil de remontée vers Home Assistant">
        <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-4">
          {POLLING_CARDS.map((p) => {
            const active = snapshot.polling_profile === p.id;
            return (
              <button
                key={p.id}
                type="button"
                onClick={async () => {
                  onSnapshot(await updateSettings({ polling_profile: p.id }));
                  onNotify(`Profil de remontée : ${p.label}`);
                }}
                className={`rounded-xl p-3.5 text-left transition ${
                  active
                    ? 'bg-neutral-900 text-white dark:bg-white dark:text-neutral-950'
                    : 'bg-[var(--bg-subtle)]/55 ring-1 ring-[var(--border-hairline)] hover:ring-[var(--border-strong)]'
                }`}
              >
                <div className="flex items-center justify-between">
                  <span className="text-xs font-semibold">{p.label}</span>
                  <span
                    className={`tabular text-[10px] ${
                      active ? 'text-white/75 dark:text-neutral-600' : 'text-[var(--text-muted)]'
                    }`}
                  >
                    {p.cadence}
                  </span>
                </div>
                <p
                  className={`mt-1.5 text-[11px] leading-relaxed ${
                    active ? 'text-white/75 dark:text-neutral-600' : 'text-[var(--text-muted)]'
                  }`}
                >
                  {p.desc}
                </p>
              </button>
            );
          })}
        </div>
      </SurfaceCard>

      {/* Granular Remote Permissions Matrix */}
      <SurfaceCard
        title="Permissions distantes accordées à Home Assistant"
        subtitle="Appliquées strictement par ctrlmated sur WebSocket et MQTT."
      >
        <div className="grid grid-cols-1 gap-2.5 md:grid-cols-2">
          {PERMISSION_ROWS.map((row) => (
            <div
              key={row.key}
              className="flex items-center justify-between gap-3 rounded-xl bg-[var(--bg-subtle)]/45 px-3.5 py-2.5 ring-1 ring-[var(--border-hairline)]"
            >
              <div className="min-w-0">
                <div className="text-xs font-medium">{row.label}</div>
                <div className="truncate text-[11px] text-[var(--text-muted)]">{row.desc}</div>
              </div>
              <ToggleSwitch
                checked={snapshot.permissions[row.key]}
                label={row.label}
                onChange={async (checked) => {
                  const next = { ...snapshot.permissions, [row.key]: checked };
                  onSnapshot(await updateSettings({ permissions: next }));
                }}
              />
            </div>
          ))}
        </div>
      </SurfaceCard>

      {/* Secondary Transport: Home Assistant 2026.11 MQTT Device Discovery */}
      <SurfaceCard
        title="Transport secondaire — MQTT Device Discovery (HA 2026.11)"
        action={
          <div className="flex items-center gap-3">
            <StatusBadge
              tone={
                snapshot.mqtt_connected
                  ? 'emerald'
                  : snapshot.mqtt.enabled
                    ? 'amber'
                    : 'neutral'
              }
              pulse={snapshot.mqtt_connected}
            >
              <Radio className="h-3 w-3" />
              {snapshot.mqtt_connected
                ? 'Broker connecté'
                : snapshot.mqtt.enabled
                  ? 'Connexion…'
                  : 'Désactivé'}
            </StatusBadge>
            <ToggleSwitch
              checked={mqttDraft.enabled}
              label="Activer MQTT Discovery"
              onChange={(enabled) => setMqttDraft({ ...mqttDraft, enabled })}
            />
          </div>
        }
      >
        <div className="space-y-3">
          <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
            <input
              value={mqttDraft.host}
              onChange={(e) => setMqttDraft({ ...mqttDraft, host: e.target.value })}
              placeholder="Hôte broker (ex. omv.home)"
              className="rounded-lg bg-[var(--bg-subtle)] px-3 py-2 text-xs ring-1 ring-[var(--border-hairline)]"
            />
            <input
              type="number"
              value={mqttDraft.port}
              onChange={(e) =>
                setMqttDraft({ ...mqttDraft, port: Number(e.target.value) || 1883 })
              }
              placeholder="Port (1883 / 8883)"
              className="tabular rounded-lg bg-[var(--bg-subtle)] px-3 py-2 text-xs ring-1 ring-[var(--border-hairline)]"
            />
            <input
              value={mqttDraft.discovery_prefix}
              onChange={(e) =>
                setMqttDraft({ ...mqttDraft, discovery_prefix: e.target.value })
              }
              placeholder="Préfixe (homeassistant)"
              className="rounded-lg bg-[var(--bg-subtle)] px-3 py-2 text-xs ring-1 ring-[var(--border-hairline)]"
            />
            <input
              value={mqttDraft.username ?? ''}
              onChange={(e) =>
                setMqttDraft({ ...mqttDraft, username: e.target.value || null })
              }
              placeholder="Utilisateur (optionnel)"
              className="rounded-lg bg-[var(--bg-subtle)] px-3 py-2 text-xs ring-1 ring-[var(--border-hairline)]"
            />
            <input
              type="password"
              value={mqttDraft.password ?? ''}
              onChange={(e) =>
                setMqttDraft({ ...mqttDraft, password: e.target.value || null })
              }
              placeholder="Mot de passe (optionnel)"
              className="rounded-lg bg-[var(--bg-subtle)] px-3 py-2 text-xs ring-1 ring-[var(--border-hairline)]"
            />
            <div className="flex items-center justify-between gap-2 rounded-lg bg-[var(--bg-subtle)]/50 px-3 py-1.5 ring-1 ring-[var(--border-hairline)]">
              <span className="text-xs text-[var(--text-secondary)]">Chiffrement TLS</span>
              <ToggleSwitch
                checked={mqttDraft.use_tls}
                label="TLS MQTT"
                onChange={(use_tls) => setMqttDraft({ ...mqttDraft, use_tls })}
              />
            </div>
          </div>

          <div className="flex justify-end">
            <button
              type="button"
              onClick={async () => {
                onSnapshot(await updateSettings({ mqtt: mqttDraft }));
                onNotify('Configuration MQTT appliquée');
              }}
              className="rounded-lg bg-neutral-900 px-4 py-1.5 text-xs font-medium text-white transition hover:opacity-90 dark:bg-white dark:text-neutral-950"
            >
              Enregistrer & appliquer
            </button>
          </div>
        </div>
      </SurfaceCard>
    </div>
  );
}
