import { useState } from 'react';
import {
  Bell,
  Lock,
  MonitorOff,
  Moon,
  Power,
  RefreshCw,
  RotateCcw,
  Snowflake,
  Download,
  type LucideIcon,
} from 'lucide-react';
import { cmd } from '../api/daemon';
import { Button } from '../components/ui/Button';
import { ConfirmDialog } from '../components/ui/Dialog';
import { SelectField, Slider, SwitchRow, TextField } from '../components/ui/Form';
import { Badge, Banner, Card, EmptyState, KeyValue, PageHeader } from '../components/ui/Layout';
import { useI18n } from '../i18n';
import { hasEntity } from '../lib/snapshot';
import { useActions } from '../store/actions';
import type { CommandResult, DaemonSnapshot, PowerButton } from '../types/protocol';
import { POWER_BUTTONS } from '../types/protocol';
import { ScriptsEditor } from './ScriptsEditor';

type ConfirmedPower = 'power_shutdown' | 'power_restart' | 'power_sleep' | 'power_hibernate';

interface LastResult {
  label: string;
  result: CommandResult | null;
}

const CONFIRMED: readonly PowerButton[] = [
  'power_shutdown',
  'power_restart',
  'power_sleep',
  'power_hibernate',
];
const isConfirmed = (b: PowerButton): b is ConfirmedPower => CONFIRMED.includes(b);

const POWER_ICON: Record<PowerButton, LucideIcon> = {
  power_shutdown: Power,
  power_restart: RotateCcw,
  power_sleep: Moon,
  power_hibernate: Snowflake,
  power_lock: Lock,
  display_off: MonitorOff,
};

/** Inline, translated outcome of the last action of a section (toasts only announce successes). */
function ResultNote({ last }: { last: LastResult | null }) {
  const { t, code } = useI18n();
  if (!last) return null;
  const r = last.result;
  const ok = r?.ok === true;
  return (
    <div className="mt-4" aria-live="polite">
      <Banner
        tone={ok ? 'success' : 'danger'}
        title={`${last.label}: ${r ? code('result', r.code) : code('result', 'internal')}`}
      >
        {r?.message ? (
          <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap font-mono text-xs text-muted">
            {r.message}
          </pre>
        ) : ok ? (
          t('system.result.ok')
        ) : null}
      </Banner>
    </div>
  );
}

function PowerCard({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const [pendingAction, setPendingAction] = useState<ConfirmedPower | null>(null);
  const [busy, setBusy] = useState(false);
  const [last, setLast] = useState<LastResult | null>(null);
  const buttons = POWER_BUTTONS.filter((b) => hasEntity(snap, b));

  const exec = async (b: PowerButton) => {
    setBusy(true);
    const r = await actions.run(cmd.press(b), { silent: true });
    setBusy(false);
    setPendingAction(null);
    setLast({ label: t(`power.${b}`), result: r });
  };

  return (
    <Card title={t('system.power.title')} description={t('system.power.description')}>
      {buttons.length === 0 ? (
        <EmptyState title={t('system.power.none')} />
      ) : (
        <div className="grid gap-2 [grid-template-columns:repeat(auto-fill,minmax(150px,1fr))]">
          {buttons.map((b) => {
            const Icon = POWER_ICON[b];
            return (
              <Button
                key={b}
                icon={<Icon className="size-4" aria-hidden />}
                className="justify-start"
                onClick={() => (isConfirmed(b) ? setPendingAction(b) : void exec(b))}
              >
                {t(`power.${b}`)}
              </Button>
            );
          })}
        </div>
      )}
      <ResultNote last={last} />
      <ConfirmDialog
        open={pendingAction !== null}
        tone="danger"
        title={pendingAction ? t(`power.${pendingAction}.confirm.title`) : ''}
        description={pendingAction ? t(`power.${pendingAction}.confirm.body`) : ''}
        confirmLabel={pendingAction ? t(`power.${pendingAction}`) : ''}
        cancelLabel={t('common.cancel')}
        loading={busy}
        onCancel={() => setPendingAction(null)}
        onConfirm={() => pendingAction && void exec(pendingAction)}
      />
    </Card>
  );
}

function AudioCard({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t, num } = useI18n();
  const actions = useActions();
  const [last, setLast] = useState<LastResult | null>(null);
  const audio = snap.session?.audio ?? null;
  const report = async (label: string, p: Promise<CommandResult | null>) =>
    setLast({ label, result: await p });

  return (
    <Card title={t('system.audio.title')}>
      {audio === null ? (
        <EmptyState title={t('system.audio.unavailable.title')}>
          {t('system.audio.unavailable.body')}
        </EmptyState>
      ) : (
        <div className="space-y-4">
          <Slider
            label={t('system.audio.volume')}
            value={Math.round(audio.volume)}
            min={0}
            max={100}
            format={(v) => `${num(v)}\u202f%`}
            onCommit={(v) =>
              void report(t('system.audio.volume'), actions.run(cmd.volume(v), { silent: true }))
            }
          />
          <SwitchRow
            label={t('system.audio.mute')}
            checked={audio.muted}
            onChange={(m) =>
              void report(t('system.audio.mute'), actions.run(cmd.mute(m), { silent: true }))
            }
          />
          {audio.outputs.length > 0 ? (
            <SelectField
              label={t('system.audio.output')}
              value={audio.output_id ?? ''}
              options={[
                ...(audio.output_id ? [] : [{ value: '', label: t('system.audio.output.none') }]),
                ...audio.outputs.map((o) => ({ value: o.id, label: o.name })),
              ]}
              onChange={(id) =>
                id &&
                void report(
                  t('system.audio.output'),
                  actions.run(cmd.audioOutput(id), { silent: true }),
                )
              }
            />
          ) : (
            <p className="text-sm text-muted">{t('system.audio.output.empty')}</p>
          )}
        </div>
      )}
      <ResultNote last={last && !last.result?.ok ? last : null} />
    </Card>
  );
}

function SessionCard({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t, num } = useI18n();
  const s = snap.session;
  const flag = (v: boolean | null) =>
    v === null ? (
      <Badge>{t('common.unknown')}</Badge>
    ) : (
      <Badge tone={v ? 'warning' : 'neutral'}>
        {v ? t('system.session.in_use') : t('system.session.idle')}
      </Badge>
    );
  return (
    <Card title={t('system.session.title')} description={t('system.session.description')}>
      {s === null ? (
        <EmptyState title={t('system.session.unavailable.title')}>
          {t('system.session.unavailable.body')}
        </EmptyState>
      ) : (
        <dl className="divide-y divide-line">
          <KeyValue label={t('system.session.app')}>
            {s.foreground_app ? (
              <span title={s.foreground_app.window_title ?? undefined}>
                {s.foreground_app.process_name}
              </span>
            ) : (
              t('common.unknown')
            )}
          </KeyValue>
          <KeyValue label={t('system.session.gaming')}>
            <Badge tone={s.gaming_mode ? 'accent' : 'neutral'}>
              {s.gaming_mode ? t('common.on') : t('common.off')}
            </Badge>
          </KeyValue>
          <KeyValue label={t('system.session.locked')}>
            {s.locked ? t('common.yes') : t('common.no')}
          </KeyValue>
          <KeyValue label={t('system.session.idle_time')}>
            {t('system.session.seconds', { count: s.idle_seconds, formatted: num(s.idle_seconds) })}
          </KeyValue>
          <KeyValue label={t('system.session.mic')}>{flag(s.mic_in_use)}</KeyValue>
          <KeyValue label={t('system.session.webcam')}>{flag(s.webcam_in_use)}</KeyValue>
        </dl>
      )}
    </Card>
  );
}

function NotifyCard() {
  const { t } = useI18n();
  const actions = useActions();
  const [title, setTitle] = useState('');
  const [message, setMessage] = useState('');
  const [last, setLast] = useState<LastResult | null>(null);
  const [busy, setBusy] = useState(false);
  const send = async () => {
    setBusy(true);
    const r = await actions.run(
      cmd.notify(
        title.trim() || t('system.notify.default_title'),
        message.trim() || t('system.notify.default_body'),
      ),
      { silent: true },
    );
    setBusy(false);
    setLast({ label: t('system.notify.title'), result: r });
  };
  return (
    <Card title={t('system.notify.title')} description={t('system.notify.description')}>
      <div className="grid gap-3 sm:grid-cols-2">
        <TextField
          label={t('system.notify.field.title')}
          value={title}
          placeholder={t('system.notify.default_title')}
          onChange={setTitle}
          maxLength={64}
        />
        <TextField
          label={t('system.notify.field.message')}
          value={message}
          placeholder={t('system.notify.default_body')}
          onChange={setMessage}
          maxLength={200}
        />
      </div>
      <div className="mt-3 flex items-center gap-3">
        <Button
          icon={<Bell className="size-4" aria-hidden />}
          loading={busy}
          onClick={() => void send()}
        >
          {t('system.notify.send')}
        </Button>
      </div>
      <ResultNote last={last} />
    </Card>
  );
}

function UpdatesCard({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t, code, ago, num } = useI18n();
  const actions = useActions();
  const u = snap.updates;
  const [confirmInstall, setConfirmInstall] = useState(false);
  const [confirmReboot, setConfirmReboot] = useState(false);
  const [last, setLast] = useState<LastResult | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = u.pending_count;
  const scan = async () => {
    setBusy(true);
    const r = await actions.run(cmd.checkOsUpdates(), { silent: true });
    setBusy(false);
    setLast({ label: t('system.updates.scan'), result: r });
  };
  const install = async () => {
    setConfirmInstall(false);
    const r = await actions.run(cmd.installOsUpdates(), { silent: true });
    setLast({ label: t('system.updates.install'), result: r });
  };
  const reboot = async () => {
    setConfirmReboot(false);
    const r = await actions.run(cmd.press('power_restart'), { silent: true });
    setLast({ label: t('power.power_restart'), result: r });
  };

  return (
    <Card
      title={t('system.updates.title')}
      description={
        u.installed_version
          ? t('system.updates.installed', { version: u.installed_version })
          : undefined
      }
      actions={
        <>
          <Button
            size="sm"
            icon={<RefreshCw className="size-4" aria-hidden />}
            loading={busy || u.in_progress}
            onClick={() => void scan()}
            disabled={u.in_progress}
          >
            {t('system.updates.scan')}
          </Button>
          <Button
            size="sm"
            variant="primary"
            icon={<Download className="size-4" aria-hidden />}
            disabled={!pending || u.in_progress}
            onClick={() => setConfirmInstall(true)}
          >
            {t('system.updates.install')}
          </Button>
        </>
      }
    >
      <div className="flex flex-wrap items-center gap-2">
        {pending === null ? (
          <Badge>{t('system.updates.not_checked')}</Badge>
        ) : pending === 0 ? (
          <Badge tone="success">{t('system.updates.up_to_date')}</Badge>
        ) : (
          <Badge tone="accent">
            {t('system.updates.pending', { count: pending, formatted: num(pending) })}
          </Badge>
        )}
        {u.in_progress ? <Badge tone="accent">{t('system.updates.in_progress')}</Badge> : null}
        {u.reboot_required ? (
          <Badge tone="warning">{t('system.updates.reboot_required')}</Badge>
        ) : null}
        {u.last_checked ? (
          <span className="text-xs text-muted">
            {t('system.updates.last_checked', { when: ago(u.last_checked) })}
          </span>
        ) : null}
      </div>

      {u.in_progress ? (
        <div
          role="progressbar"
          aria-label={t('system.updates.in_progress')}
          className="mt-3 h-1.5 overflow-hidden rounded-full bg-surface-2"
        >
          <div className="cm-spin-bar h-full w-1/3 rounded-full bg-accent" />
        </div>
      ) : null}

      {u.error ? (
        <div className="mt-3">
          <Banner tone="danger" role="alert" title={t('system.updates.error')}>
            {code('result', u.error)}
          </Banner>
        </div>
      ) : null}

      {u.pending.length > 0 ? (
        <ul
          className="mt-4 divide-y divide-line rounded-control border border-line"
          aria-label={t('system.updates.list')}
        >
          {u.pending.map((p, i) => (
            <li
              key={`${p.kb ?? p.title}-${i}`}
              className="flex items-start justify-between gap-3 px-3 py-2 text-sm"
            >
              <span className="min-w-0 text-fg">{p.title}</span>
              {p.kb ? (
                <span className="shrink-0 rounded bg-surface-2 px-1.5 py-0.5 font-mono text-xs text-muted">
                  {p.kb}
                </span>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}

      {u.reboot_required ? (
        <div className="mt-3">
          <Button
            onClick={() => setConfirmReboot(true)}
            icon={<RotateCcw className="size-4" aria-hidden />}
          >
            {t('power.power_restart')}
          </Button>
        </div>
      ) : null}
      <ResultNote last={last} />

      <ConfirmDialog
        open={confirmInstall}
        title={t('system.updates.install.confirm.title')}
        description={t('system.updates.install.confirm.body', { count: pending ?? 0 })}
        confirmLabel={t('system.updates.install')}
        cancelLabel={t('common.cancel')}
        onCancel={() => setConfirmInstall(false)}
        onConfirm={() => void install()}
      />
      <ConfirmDialog
        open={confirmReboot}
        tone="danger"
        title={t('power.power_restart.confirm.title')}
        description={t('power.power_restart.confirm.body')}
        confirmLabel={t('power.power_restart')}
        cancelLabel={t('common.cancel')}
        onCancel={() => setConfirmReboot(false)}
        onConfirm={() => void reboot()}
      />
    </Card>
  );
}

export function System({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t } = useI18n();
  const [scriptResult, setScriptResult] = useState<LastResult | null>(null);
  return (
    <>
      <PageHeader title={t('system.title')} description={t('system.description')} />
      {!snap.service.agent_connected && !snap.service.in_process_session ? (
        <div className="mb-4">
          <Banner tone="warning" title={t('system.noagent.title')}>
            {t('system.noagent.body')}
          </Banner>
        </div>
      ) : null}
      <div className="space-y-4">
        <PowerCard snapshot={snap} />
        <div className="grid gap-4 xl:grid-cols-2">
          <AudioCard snapshot={snap} />
          <SessionCard snapshot={snap} />
        </div>
        <NotifyCard />
        <UpdatesCard snapshot={snap} />
        <ScriptsEditor
          snapshot={snap}
          onResult={(label, result) => setScriptResult({ label, result })}
        />
        <ResultNote last={scriptResult} />
      </div>
    </>
  );
}
