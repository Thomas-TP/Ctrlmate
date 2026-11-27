import { useState } from 'react';
import {
  Bell,
  Lock,
  MonitorOff,
  Moon,
  Play,
  Power,
  RotateCcw,
  Snowflake,
  type LucideIcon,
} from 'lucide-react';
import { cmd } from '../../../api/daemon';
import { useI18n } from '../../../i18n';
import { hasEntity } from '../../../lib/snapshot';
import { useActions } from '../../../store/actions';
import {
  POWER_BUTTONS,
  type CommandResult,
  type DaemonSnapshot,
  type PowerButton,
} from '../../../types/protocol';
import {
  Badge,
  Banner,
  Button,
  EmptyNote,
  KeyValue,
  SectionTitle,
  SelectField,
  Slider,
  SwitchRow,
  TextField,
} from '../../../ui/kit';
import { ConfirmDialog } from '../../../ui/overlay';

type Confirmed = 'power_shutdown' | 'power_restart' | 'power_sleep' | 'power_hibernate';
const CONFIRMED: readonly PowerButton[] = [
  'power_shutdown',
  'power_restart',
  'power_sleep',
  'power_hibernate',
];
const isConfirmed = (b: PowerButton): b is Confirmed => CONFIRMED.includes(b);

const ICON: Record<PowerButton, LucideIcon> = {
  power_shutdown: Power,
  power_restart: RotateCcw,
  power_sleep: Moon,
  power_hibernate: Snowflake,
  power_lock: Lock,
  display_off: MonitorOff,
};

interface Last {
  label: string;
  result: CommandResult | null;
}

/** Inline outcome for actions whose result matters (script output, notification, errors). */
export function ResultNote({ last }: { last: Last | null }) {
  const { t, code } = useI18n();
  if (!last) return null;
  const ok = last.result?.ok === true;
  return (
    <div className="mt-3" aria-live="polite">
      <Banner
        tone={ok ? 'success' : 'danger'}
        title={`${last.label}: ${code('result', last.result?.code ?? 'internal')}`}
      >
        {last.result?.message ? (
          <pre className="mt-1 max-h-32 overflow-auto font-mono text-xs whitespace-pre-wrap">
            {last.result.message}
          </pre>
        ) : ok ? (
          t('system.result.ok')
        ) : null}
      </Banner>
    </div>
  );
}

export function SystemSheet({ snap }: { snap: DaemonSnapshot }) {
  const { t, num } = useI18n();
  const actions = useActions();
  const [pending, setPending] = useState<Confirmed | null>(null);
  const [busy, setBusy] = useState(false);
  const [last, setLast] = useState<Last | null>(null);
  const [title, setTitle] = useState('');
  const [message, setMessage] = useState('');

  const buttons = POWER_BUTTONS.filter((b) => hasEntity(snap, b));
  const scripts = snap.manifest.entities.filter(
    (e) => e.platform === 'button' && e.id.startsWith('script_'),
  );
  const audio = snap.session?.audio ?? null;
  const s = snap.session;

  const exec = async (b: PowerButton) => {
    setBusy(true);
    const r = await actions.run(cmd.press(b), { silent: true });
    setBusy(false);
    setPending(null);
    setLast({ label: t(`power.${b}`), result: r });
  };
  const flag = (v: boolean | null) =>
    v === null ? (
      <Badge>{t('common.unknown')}</Badge>
    ) : (
      <Badge tone={v ? 'warning' : 'neutral'}>
        {v ? t('system.session.in_use') : t('system.session.idle')}
      </Badge>
    );

  return (
    <div>
      {!snap.service.agent_connected && !snap.service.in_process_session ? (
        <div className="mb-4">
          <Banner tone="warning" title={t('system.noagent.title')}>
            {t('system.noagent.body')}
          </Banner>
        </div>
      ) : null}

      <SectionTitle>{t('system.power.title')}</SectionTitle>
      {buttons.length === 0 ? (
        <EmptyNote title={t('system.power.none')} />
      ) : (
        <div className="grid grid-cols-2 gap-2">
          {buttons.map((b) => {
            const Icon = ICON[b];
            return (
              <Button
                key={b}
                icon={<Icon className="size-4" aria-hidden />}
                className="justify-start"
                onClick={() => (isConfirmed(b) ? setPending(b) : void exec(b))}
              >
                {t(`power.${b}`)}
              </Button>
            );
          })}
        </div>
      )}

      <SectionTitle>{t('system.audio.title')}</SectionTitle>
      {audio === null ? (
        <EmptyNote title={t('system.audio.unavailable.title')}>
          {t('system.audio.unavailable.body')}
        </EmptyNote>
      ) : (
        <div className="space-y-3">
          <Slider
            label={t('system.audio.volume')}
            value={Math.round(audio.volume)}
            min={0}
            max={100}
            format={(v) => `${num(v)}\u202f%`}
            onCommit={(v) => void actions.run(cmd.volume(v))}
          />
          <SwitchRow
            label={t('system.audio.mute')}
            checked={audio.muted}
            onChange={(m) => void actions.run(cmd.mute(m))}
          />
          {audio.outputs.length > 0 ? (
            <SelectField
              label={t('system.audio.output')}
              value={audio.output_id ?? ''}
              options={[
                ...(audio.output_id ? [] : [{ value: '', label: t('system.audio.output.none') }]),
                ...audio.outputs.map((o) => ({ value: o.id, label: o.name })),
              ]}
              onChange={(id) => id && void actions.run(cmd.audioOutput(id))}
            />
          ) : null}
        </div>
      )}

      <SectionTitle>{t('system.session.title')}</SectionTitle>
      {s === null ? (
        <EmptyNote title={t('system.session.unavailable.title')}>
          {t('system.session.unavailable.body')}
        </EmptyNote>
      ) : (
        <dl className="divide-y divide-line rounded-xl border border-line px-4">
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

      <SectionTitle>{t('system.notify.title')}</SectionTitle>
      <div className="space-y-3">
        <TextField
          label={t('system.notify.field.title')}
          value={title}
          placeholder={t('system.notify.default_title')}
          onChange={setTitle}
        />
        <TextField
          label={t('system.notify.field.message')}
          value={message}
          placeholder={t('system.notify.default_body')}
          onChange={setMessage}
        />
        <Button
          icon={<Bell className="size-4" aria-hidden />}
          onClick={() =>
            void (async () => {
              const r = await actions.run(
                cmd.notify(
                  title.trim() || t('system.notify.default_title'),
                  message.trim() || t('system.notify.default_body'),
                ),
                { silent: true },
              );
              setLast({ label: t('system.notify.title'), result: r });
            })()
          }
        >
          {t('system.notify.send')}
        </Button>
      </div>

      {scripts.length > 0 ? (
        <>
          <SectionTitle>{t('scripts.title')}</SectionTitle>
          <div className="flex flex-wrap gap-2">
            {scripts.map((e) => (
              <Button
                key={e.id}
                icon={<Play className="size-4" aria-hidden />}
                disabled={!snap.permissions.custom_scripts}
                onClick={() =>
                  void (async () => {
                    const r = await actions.run(cmd.press(e.id), { silent: true });
                    setLast({ label: e.name, result: r });
                  })()
                }
              >
                {e.name}
              </Button>
            ))}
          </div>
        </>
      ) : null}
      <ResultNote last={last} />

      <ConfirmDialog
        open={pending !== null}
        tone="danger"
        title={pending ? t(`power.${pending}.confirm.title`) : ''}
        description={pending ? t(`power.${pending}.confirm.body`) : ''}
        confirmLabel={pending ? t(`power.${pending}`) : ''}
        loading={busy}
        onCancel={() => setPending(null)}
        onConfirm={() => pending && void exec(pending)}
      />
    </div>
  );
}
