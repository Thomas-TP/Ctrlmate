import { useState } from 'react';
import { Link2Off } from 'lucide-react';
import { daemon } from '../../../api/daemon';
import { useI18n } from '../../../i18n';
import { useActions } from '../../../store/actions';
import {
  PERMISSION_KEYS,
  type DaemonSnapshot,
  type MqttConfig,
  type PairedClientInfo,
  type PermissionKey,
} from '../../../types/protocol';
import { Badge, Button, EmptyNote, SectionTitle, SwitchRow, TextField } from '../../../ui/kit';
import { ConfirmDialog } from '../../../ui/overlay';
import { PairingPanel } from '../../shared/Pairing';

function Clients({ snap }: { snap: DaemonSnapshot }) {
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
  if (snap.paired_clients.length === 0) {
    return <EmptyNote title={t('ha.clients.empty.title')}>{t('ha.clients.empty.body')}</EmptyNote>;
  }
  return (
    <>
      <ul className="divide-y divide-line rounded-xl border border-line px-4">
        {snap.paired_clients.map((c) => (
          <li key={c.client_id} className="flex flex-wrap items-center justify-between gap-3 py-3">
            <div className="min-w-0">
              <p className="truncate text-sm font-medium text-fg">{c.client_name}</p>
              <p className="text-xs text-muted">
                {t('ha.clients.paired', { date: date(c.paired_at) })}
              </p>
            </div>
            <div className="flex items-center gap-2">
              <Badge tone={c.connected ? 'success' : 'neutral'} dot>
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
      <ConfirmDialog
        open={target !== null}
        tone="danger"
        title={t('ha.clients.revoke.title', { name: target?.client_name ?? '' })}
        description={t('ha.clients.revoke.body')}
        confirmLabel={t('ha.clients.revoke')}
        loading={busy}
        onCancel={() => setTarget(null)}
        onConfirm={() => void revoke()}
      />
    </>
  );
}

function Permissions({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const [pending, setPending] = useState<PermissionKey | null>(null);
  const toggle = async (key: PermissionKey, value: boolean) => {
    setPending(key);
    await actions.settings({ permissions: { ...snap.permissions, [key]: value } });
    setPending(null);
  };
  return (
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
          checked={snap.permissions[k]}
          disabled={pending === k}
          onChange={(v) => void toggle(k, v)}
        />
      ))}
    </div>
  );
}

type PasswordMode = 'keep' | 'set' | 'clear';

function Mqtt({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const [cfg, setCfg] = useState<MqttConfig>(snap.mqtt);
  const [password, setPassword] = useState('');
  const [mode, setMode] = useState<PasswordMode>('keep');
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  if (!dirty && cfg !== snap.mqtt) setCfg(snap.mqtt);
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
    <div className="space-y-3">
      <SwitchRow
        label={t('mqtt.enabled')}
        description={t('mqtt.description')}
        checked={cfg.enabled}
        onChange={(enabled) => patch({ enabled })}
      />
      <div className="grid grid-cols-[1fr_110px] gap-3">
        <TextField
          label={t('mqtt.host')}
          value={cfg.host}
          error={hostError}
          onChange={(host) => patch({ host })}
        />
        <TextField
          label={t('mqtt.port')}
          value={String(cfg.port)}
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
      <div className="grid grid-cols-2 gap-3">
        <TextField
          label={t('mqtt.username')}
          value={cfg.username ?? ''}
          onChange={(username) => patch({ username })}
        />
        <div>
          <TextField
            label={t('mqtt.password')}
            type="password"
            value={password}
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
      </div>
      <TextField
        label={t('mqtt.prefix')}
        value={cfg.discovery_prefix}
        mono
        error={prefixError}
        onChange={(discovery_prefix) => patch({ discovery_prefix })}
      />
      <div className="flex gap-2 pt-1">
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
    </div>
  );
}

export function HaSheet({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  return (
    <div>
      <SectionTitle>{t('ha.pairing.title')}</SectionTitle>
      <p className="mb-3 text-[13px] text-muted">{t('ha.pairing.description')}</p>
      <PairingPanel snap={snap} />

      <SectionTitle>{t('ha.clients.title')}</SectionTitle>
      <Clients snap={snap} />

      <SectionTitle>{t('ha.permissions.title')}</SectionTitle>
      <p className="text-[13px] text-muted">{t('ha.permissions.description')}</p>
      <Permissions snap={snap} />

      <details className="mt-6 rounded-xl border border-line px-4 py-3">
        <summary className="flex cursor-pointer items-center justify-between text-sm font-medium text-fg">
          {t('mqtt.title')}
          <Badge
            tone={snap.mqtt.enabled ? (snap.mqtt_connected ? 'success' : 'warning') : 'neutral'}
          >
            {snap.mqtt.enabled
              ? snap.mqtt_connected
                ? t('mqtt.connected')
                : t('mqtt.disconnected')
              : t('mqtt.disabled')}
          </Badge>
        </summary>
        <div className="pt-4">
          <Mqtt snap={snap} />
        </div>
      </details>
    </div>
  );
}
