import { useEffect, useState } from 'react';
import { Copy, KeyRound, Link2Off } from 'lucide-react';
import { daemon } from '../api/daemon';
import { Button, cn } from '../components/ui/Button';
import { ConfirmDialog } from '../components/ui/Dialog';
import { SwitchRow, TextField } from '../components/ui/Form';
import { Badge, Banner, Card, EmptyState, KeyValue, PageHeader } from '../components/ui/Layout';
import { useNow } from '../hooks/hooks';
import { useI18n } from '../i18n';
import { copyText } from '../lib/clipboard';
import { formatCountdown, groupFingerprint } from '../lib/format';
import { useActions } from '../store/actions';
import { useDaemon } from '../store/daemon';
import {
  PERMISSION_KEYS,
  POLLING_PROFILES,
  type DaemonSnapshot,
  type MqttConfig,
  type PairedClientInfo,
  type PermissionKey,
  type PollingProfile,
} from '../types/protocol';

function Fingerprint({ hex }: { hex: string }) {
  const { t } = useI18n();
  const pushToast = useDaemon((s) => s.pushToast);
  const groups = groupFingerprint(hex);
  if (groups.length === 0) return <p className="text-sm text-muted">{t('common.unavailable')}</p>;
  const copy = async () => {
    const ok = await copyText(groups.join(''));
    pushToast(ok ? 'success' : 'error', ok ? t('common.copied') : t('common.copy_failed'));
  };
  return (
    <div>
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-xs font-medium text-muted">{t('ha.fingerprint.title')}</h3>
        <Button
          size="sm"
          variant="ghost"
          icon={<Copy className="size-4" aria-hidden />}
          onClick={() => void copy()}
        >
          {t('common.copy')}
        </Button>
      </div>
      <p
        className="mt-2 grid grid-cols-4 gap-x-3 gap-y-1 font-mono text-[13px] text-fg sm:grid-cols-8"
        aria-label={t('ha.fingerprint.aria', { value: groups.join(' ') })}
      >
        {groups.map((g, i) => (
          <span key={i} aria-hidden>
            {g}
          </span>
        ))}
      </p>
      <p className="mt-2 text-xs text-subtle">{t('ha.fingerprint.hint')}</p>
    </div>
  );
}

function PairingCard({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const p = snap.pairing;
  const now = useNow(1000, p.active);
  const [busy, setBusy] = useState(false);
  const remaining = p.expires_at ? new Date(p.expires_at).getTime() - now : 0;
  const live = p.active && p.pin !== null && remaining > 0;

  const start = async () => {
    setBusy(true);
    await actions.snapshotCall(() => daemon.startPairing());
    setBusy(false);
  };
  const cancel = async () => {
    setBusy(true);
    await actions.snapshotCall(() => daemon.cancelPairing());
    setBusy(false);
  };

  const pin = p.pin ?? '';
  const grouped = pin.length === 6 ? `${pin.slice(0, 3)} ${pin.slice(3)}` : pin;
  return (
    <Card title={t('ha.pairing.title')} description={t('ha.pairing.description')}>
      {live ? (
        <div className="space-y-5">
          <div className="flex flex-wrap items-center justify-between gap-4">
            <div>
              <p className="text-xs font-medium text-muted">{t('ha.pairing.pin')}</p>
              <output
                aria-label={t('ha.pairing.pin.aria', { pin: pin.split('').join(' ') })}
                className="mt-1 block font-mono text-5xl font-semibold tracking-[0.12em] text-fg tabular-nums"
              >
                {grouped}
              </output>
            </div>
            <div className="text-right">
              <p className="text-xs font-medium text-muted">{t('ha.pairing.expires')}</p>
              <p
                className="mt-1 font-mono text-2xl text-fg tabular-nums"
                aria-label={t('ha.pairing.countdown.aria', { time: formatCountdown(remaining) })}
              >
                {formatCountdown(remaining)}
              </p>
              <p className="mt-1 text-xs text-subtle">
                {t('ha.pairing.port', { port: p.listen_port })}
              </p>
            </div>
          </div>
          <Fingerprint hex={p.cert_fingerprint} />
          <div className="flex gap-2">
            <Button onClick={() => void cancel()} loading={busy}>
              {t('ha.pairing.cancel')}
            </Button>
          </div>
        </div>
      ) : (
        <div className="space-y-5">
          {p.active ? (
            <Banner tone="warning">{t('ha.pairing.expired')}</Banner>
          ) : (
            <p className="text-sm text-muted">{t('ha.pairing.idle')}</p>
          )}
          <div>
            <Button
              variant="primary"
              icon={<KeyRound className="size-4" aria-hidden />}
              onClick={() => void start()}
              loading={busy}
            >
              {t('ha.pairing.start')}
            </Button>
          </div>
          <Fingerprint hex={p.cert_fingerprint} />
        </div>
      )}
    </Card>
  );
}

function ClientsCard({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t, date } = useI18n();
  const actions = useActions();
  const [target, setTarget] = useState<PairedClientInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const revoke = async () => {
    if (!target) return;
    setBusy(true);
    await actions.snapshotCall(() => daemon.revokeClient(target.client_id), {
      successKey: 'ha.clients.revoked',
    });
    setBusy(false);
    setTarget(null);
  };
  return (
    <Card title={t('ha.clients.title')} description={t('ha.clients.description')}>
      {snap.paired_clients.length === 0 ? (
        <EmptyState title={t('ha.clients.empty.title')}>{t('ha.clients.empty.body')}</EmptyState>
      ) : (
        <ul className="divide-y divide-line">
          {snap.paired_clients.map((c) => (
            <li
              key={c.client_id}
              className="flex flex-wrap items-center justify-between gap-3 py-3"
            >
              <div className="min-w-0">
                <p className="truncate text-sm font-medium text-fg">{c.client_name}</p>
                <p className="text-xs text-muted">
                  {t('ha.clients.paired', { date: date(c.paired_at) })}
                </p>
              </div>
              <div className="flex items-center gap-3">
                <Badge tone={c.connected ? 'success' : 'neutral'}>
                  {c.connected
                    ? t('ha.clients.live', { count: c.connections })
                    : t('ha.clients.offline')}
                </Badge>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Link2Off className="size-4" aria-hidden />}
                  onClick={() => setTarget(c)}
                >
                  {t('ha.clients.revoke')}
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}
      <ConfirmDialog
        open={target !== null}
        tone="danger"
        title={t('ha.clients.revoke.title', { name: target?.client_name ?? '' })}
        description={t('ha.clients.revoke.body')}
        confirmLabel={t('ha.clients.revoke')}
        cancelLabel={t('common.cancel')}
        loading={busy}
        onCancel={() => setTarget(null)}
        onConfirm={() => void revoke()}
      />
    </Card>
  );
}

function PermissionsCard({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const [pending, setPending] = useState<PermissionKey | null>(null);
  const toggle = async (key: PermissionKey, value: boolean) => {
    setPending(key);
    await actions.settings({ permissions: { ...snap.permissions, [key]: value } });
    setPending(null);
  };
  return (
    <Card title={t('ha.permissions.title')} description={t('ha.permissions.description')}>
      <div className="divide-y divide-line">
        {PERMISSION_KEYS.map((k) => (
          <SwitchRow
            key={k}
            label={t(`perm.${k}`)}
            description={
              <>
                {t(`perm.${k}.desc`)}
                {k === 'custom_scripts' ? ` ${t('perm.custom_scripts.warning')}` : ''}
              </>
            }
            tone={k === 'custom_scripts' ? 'warning' : 'default'}
            checked={snap.permissions[k]}
            disabled={pending === k}
            onChange={(v) => void toggle(k, v)}
          />
        ))}
      </div>
    </Card>
  );
}

type PasswordMode = 'keep' | 'set' | 'clear';

function MqttCard({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const [cfg, setCfg] = useState<MqttConfig>(snap.mqtt);
  const [password, setPassword] = useState('');
  const [mode, setMode] = useState<PasswordMode>('keep');
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  // Follow the daemon (e.g. connection state changes) unless the user is editing.
  useEffect(() => {
    if (!dirty) setCfg(snap.mqtt);
  }, [snap.mqtt, dirty]);
  const patch = (p: Partial<MqttConfig>) => {
    setDirty(true);
    setCfg((c) => ({ ...c, ...p }));
  };

  const hostError = cfg.enabled && cfg.host.trim() === '' ? t('mqtt.error.host') : undefined;
  const portError =
    !Number.isInteger(cfg.port) || cfg.port < 1 || cfg.port > 65535
      ? t('mqtt.error.port')
      : undefined;
  const prefixError =
    cfg.discovery_prefix.trim() === '' || /\s/.test(cfg.discovery_prefix)
      ? t('mqtt.error.prefix')
      : undefined;
  const invalid = Boolean(hostError ?? portError ?? prefixError);

  const save = async () => {
    setSaving(true);
    // `password: null` keeps the stored secret, `""` clears it.
    const pw = mode === 'clear' ? '' : mode === 'set' ? password : null;
    const out = await actions.settings(
      {
        mqtt: {
          ...cfg,
          host: cfg.host.trim(),
          username: cfg.username?.trim() ? cfg.username.trim() : null,
          password: pw,
        },
      },
      { successKey: 'mqtt.saved' },
    );
    setSaving(false);
    if (out) {
      setDirty(false);
      setPassword('');
      setMode('keep');
      setCfg(out.mqtt);
    }
  };

  return (
    <Card
      title={t('mqtt.title')}
      description={t('mqtt.description')}
      actions={
        <Badge tone={snap.mqtt.enabled ? (snap.mqtt_connected ? 'success' : 'warning') : 'neutral'}>
          {snap.mqtt.enabled
            ? snap.mqtt_connected
              ? t('mqtt.connected')
              : t('mqtt.disconnected')
            : t('mqtt.disabled')}
        </Badge>
      }
    >
      <SwitchRow
        label={t('mqtt.enabled')}
        checked={cfg.enabled}
        onChange={(enabled) => patch({ enabled })}
      />
      <div className="mt-2 grid gap-3 sm:grid-cols-[1fr_120px]">
        <TextField
          label={t('mqtt.host')}
          value={cfg.host}
          error={hostError}
          onChange={(host) => patch({ host })}
          autoComplete="off"
        />
        <TextField
          label={t('mqtt.port')}
          value={String(cfg.port)}
          inputMode="numeric"
          error={portError}
          onChange={(v) => patch({ port: v === '' ? 0 : Number(v.replace(/\D/g, '')) })}
        />
      </div>
      <SwitchRow
        label={t('mqtt.tls')}
        description={t('mqtt.tls.desc')}
        checked={cfg.use_tls}
        onChange={(use_tls) => patch({ use_tls })}
      />
      <div className="grid gap-3 sm:grid-cols-2">
        <TextField
          label={t('mqtt.username')}
          value={cfg.username ?? ''}
          onChange={(username) => patch({ username })}
          autoComplete="off"
        />
        <div>
          <TextField
            label={t('mqtt.password')}
            type="password"
            value={password}
            autoComplete="new-password"
            placeholder={
              snap.mqtt_password_set && mode !== 'clear' ? t('mqtt.password.stored') : ''
            }
            hint={mode === 'clear' ? t('mqtt.password.will_clear') : t('mqtt.password.hint')}
            onChange={(v) => {
              setPassword(v);
              setMode(v === '' ? 'keep' : 'set');
              setDirty(true);
            }}
          />
          {snap.mqtt_password_set && mode !== 'clear' ? (
            <Button
              size="sm"
              variant="ghost"
              className="mt-1"
              onClick={() => {
                setPassword('');
                setMode('clear');
                setDirty(true);
              }}
            >
              {t('mqtt.password.clear')}
            </Button>
          ) : null}
        </div>
        <TextField
          className="sm:col-span-2"
          label={t('mqtt.prefix')}
          value={cfg.discovery_prefix}
          mono
          error={prefixError}
          onChange={(discovery_prefix) => patch({ discovery_prefix })}
        />
      </div>
      <div className="mt-4 flex gap-2">
        <Button
          variant="primary"
          disabled={!dirty || invalid}
          loading={saving}
          onClick={() => void save()}
        >
          {t('common.save')}
        </Button>
        {dirty ? (
          <Button
            variant="ghost"
            onClick={() => {
              setCfg(snap.mqtt);
              setPassword('');
              setMode('keep');
              setDirty(false);
            }}
          >
            {t('common.revert')}
          </Button>
        ) : null}
      </div>
    </Card>
  );
}

function PollingCard({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const [pending, setPending] = useState<PollingProfile | null>(null);
  const choose = async (p: PollingProfile) => {
    if (p === snap.polling_profile) return;
    setPending(p);
    await actions.settings({ polling_profile: p });
    setPending(null);
  };
  return (
    <Card title={t('ha.polling.title')} description={t('ha.polling.description')}>
      <div
        role="radiogroup"
        aria-label={t('ha.polling.title')}
        className="grid gap-3 sm:grid-cols-2"
      >
        {POLLING_PROFILES.map((p) => {
          const on = (pending ?? snap.polling_profile) === p;
          return (
            <button
              key={p}
              type="button"
              role="radio"
              aria-checked={on}
              aria-labelledby={`polling-${p}-n`}
              aria-describedby={`polling-${p}-d`}
              onClick={() => void choose(p)}
              className={cn(
                'rounded-control border p-3 text-left transition-colors',
                on ? 'border-accent bg-accent-soft' : 'border-line hover:bg-surface-2',
              )}
            >
              <span id={`polling-${p}-n`} className="block text-sm font-semibold text-fg">
                {t(`polling.${p}`)}
              </span>
              <span id={`polling-${p}-d`} className="mt-1 block text-xs text-muted">
                {t(`polling.${p}.desc`)}
              </span>
            </button>
          );
        })}
      </div>
      <dl className="mt-3">
        <KeyValue label={t('ha.polling.sampling')}>{t('ha.polling.sampling.value')}</KeyValue>
      </dl>
    </Card>
  );
}

export function HomeAssistant({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t } = useI18n();
  return (
    <>
      <PageHeader title={t('ha.title')} description={t('ha.description')} />
      {!snap.permissions.telemetry ? (
        <div className="mb-4">
          <Banner tone="warning">{t('ha.telemetry.off')}</Banner>
        </div>
      ) : null}
      <div className="space-y-4">
        <PairingCard snapshot={snap} />
        <ClientsCard snapshot={snap} />
        <PermissionsCard snapshot={snap} />
        <PollingCard snapshot={snap} />
        <MqttCard snapshot={snap} />
      </div>
    </>
  );
}
