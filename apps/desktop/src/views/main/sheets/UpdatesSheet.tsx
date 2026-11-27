import { useEffect, useState } from 'react';
import { Download, RefreshCw, RotateCcw } from 'lucide-react';
import { app, listenUpdateProgress } from '../../../api/app';
import { cmd } from '../../../api/daemon';
import { useAppUpdate } from '../../../hooks/hooks';
import { useI18n } from '../../../i18n';
import { describeError, useActions } from '../../../store/actions';
import { useDaemon } from '../../../store/daemon';
import type { DaemonSnapshot } from '../../../types/protocol';
import { Badge, Banner, Button, SectionTitle, Spinner } from '../../../ui/kit';
import { ConfirmDialog } from '../../../ui/overlay';
import { ResultNote } from './SystemSheet';

function AppUpdate() {
  const { t, code } = useI18n();
  const status = useAppUpdate();
  const pushToast = useDaemon((s) => s.pushToast);
  const [confirm, setConfirm] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const u = status.data;

  useEffect(() => {
    let off: (() => void) | undefined;
    let disposed = false;
    void listenUpdateProgress((p) => setProgress(p)).then((fn) => {
      if (disposed) fn();
      else off = fn;
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, []);

  const install = async () => {
    setConfirm(false);
    setProgress(0);
    try {
      await app.updateInstall();
    } catch (e) {
      setProgress(null);
      pushToast('error', describeError(code, e));
    }
  };

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <div className="text-sm">
          <p className="font-medium text-fg">Ctrlmate {u?.current_version ?? ''}</p>
          <p className="text-[13px] text-muted">
            {status.loading && !u ? (
              <Spinner label={t('common.loading')} />
            ) : u?.state === 'available' ? (
              t('settings.update.available', { version: u.latest_version ?? '' })
            ) : u?.state === 'up_to_date' ? (
              t('settings.update.up_to_date')
            ) : u?.state === 'unsupported' ? (
              t('settings.update.unsupported')
            ) : (
              t('settings.update.error')
            )}
          </p>
        </div>
        {u?.state === 'available' ? (
          <Button
            variant="primary"
            icon={<Download className="size-4" aria-hidden />}
            loading={progress !== null}
            onClick={() => setConfirm(true)}
          >
            {t('settings.update.install.short')}
          </Button>
        ) : (
          <Button
            icon={<RefreshCw className="size-4" aria-hidden />}
            loading={status.loading}
            onClick={status.reload}
          >
            {t('settings.update.check')}
          </Button>
        )}
      </div>
      {progress !== null ? (
        <div
          role="progressbar"
          aria-label={t('settings.update.progress')}
          aria-valuenow={progress}
          aria-valuemin={0}
          aria-valuemax={100}
          className="h-1.5 overflow-hidden rounded-full bg-surface-2"
        >
          <div className="h-full bg-accent transition-[width]" style={{ width: `${progress}%` }} />
        </div>
      ) : null}
      <ConfirmDialog
        open={confirm}
        title={t('settings.update.confirm.title', { version: u?.latest_version ?? '' })}
        description={t('settings.update.confirm.body')}
        confirmLabel={t('settings.update.install.short')}
        onCancel={() => setConfirm(false)}
        onConfirm={() => void install()}
      />
    </div>
  );
}

export function UpdatesSheet({ snap }: { snap: DaemonSnapshot }) {
  const { t, num, ago } = useI18n();
  const actions = useActions();
  const u = snap.updates;
  const [confirm, setConfirm] = useState<'install' | 'reboot' | null>(null);
  const [busy, setBusy] = useState(false);
  const [last, setLast] = useState<{
    label: string;
    result: Awaited<ReturnType<typeof actions.run>>;
  } | null>(null);
  const pending = u.pending_count ?? 0;

  const scan = async () => {
    setBusy(true);
    const r = await actions.run(cmd.checkOsUpdates(), { silent: true });
    setBusy(false);
    setLast({ label: t('system.updates.scan'), result: r });
  };
  const install = async () => {
    setConfirm(null);
    const r = await actions.run(cmd.installOsUpdates(), { silent: true });
    setLast({ label: t('system.updates.install'), result: r });
  };
  const reboot = async () => {
    setConfirm(null);
    const r = await actions.run(cmd.press('power_restart'), { silent: true });
    setLast({ label: t('power.power_restart'), result: r });
  };

  return (
    <div>
      <SectionTitle>Ctrlmate</SectionTitle>
      <AppUpdate />

      <SectionTitle>{t('system.updates.title')}</SectionTitle>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        {!u.checked ? (
          <Badge>{t('system.updates.not_checked')}</Badge>
        ) : pending === 0 ? (
          <Badge tone="success">{t('system.updates.up_to_date')}</Badge>
        ) : (
          <Badge tone="warning">
            {t('system.updates.pending', { count: pending, formatted: num(pending) })}
          </Badge>
        )}
        {u.in_progress ? <Badge tone="accent">{t('system.updates.in_progress')}</Badge> : null}
        {u.reboot_required ? (
          <Badge tone="warning">{t('system.updates.reboot_required')}</Badge>
        ) : null}
        {u.last_checked ? (
          <span className="text-xs text-subtle">
            {t('system.updates.last_checked', { when: ago(u.last_checked) })}
          </span>
        ) : null}
      </div>
      {u.installed_version ? (
        <p className="mb-3 text-[13px] text-muted">
          {t('system.updates.installed', { version: u.installed_version })}
        </p>
      ) : null}
      {u.error ? (
        <div className="mb-3">
          <Banner tone="danger" role="alert" title={t('system.updates.error')}>
            {u.error}
          </Banner>
        </div>
      ) : null}
      {u.pending.length > 0 ? (
        <ul
          aria-label={t('system.updates.list')}
          className="mb-4 divide-y divide-line rounded-xl border border-line px-4 text-sm"
        >
          {u.pending.map((p, i) => (
            <li key={`${p.title}-${i}`} className="py-2">
              {p.title}
              {p.kb ? <span className="ml-2 text-xs text-subtle">{p.kb}</span> : null}
            </li>
          ))}
        </ul>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button
          icon={<RefreshCw className="size-4" aria-hidden />}
          loading={busy || u.in_progress}
          onClick={() => void scan()}
        >
          {t('system.updates.scan')}
        </Button>
        {pending > 0 ? (
          <Button
            variant="primary"
            icon={<Download className="size-4" aria-hidden />}
            disabled={u.in_progress}
            onClick={() => setConfirm('install')}
          >
            {t('system.updates.install')}
          </Button>
        ) : null}
        {u.reboot_required ? (
          <Button
            variant="danger"
            icon={<RotateCcw className="size-4" aria-hidden />}
            onClick={() => setConfirm('reboot')}
          >
            {t('power.power_restart')}
          </Button>
        ) : null}
      </div>
      <ResultNote last={last} />

      <ConfirmDialog
        open={confirm === 'install'}
        title={t('system.updates.install.confirm.title')}
        description={t('system.updates.install.confirm.body', { count: pending })}
        confirmLabel={t('system.updates.install')}
        onCancel={() => setConfirm(null)}
        onConfirm={() => void install()}
      />
      <ConfirmDialog
        open={confirm === 'reboot'}
        tone="danger"
        title={t('power.power_restart.confirm.title')}
        description={t('power.power_restart.confirm.body')}
        confirmLabel={t('power.power_restart')}
        onCancel={() => setConfirm(null)}
        onConfirm={() => void reboot()}
      />
    </div>
  );
}
