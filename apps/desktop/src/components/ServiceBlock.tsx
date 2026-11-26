import { useState } from 'react';
import { Download, RefreshCw, Wrench } from 'lucide-react';
import { app, type ServiceState } from '../api/app';
import { useServiceStatus } from '../hooks/hooks';
import { useI18n } from '../i18n';
import { describeError } from '../store/actions';
import { useDaemon } from '../store/daemon';
import { Badge, Banner, KeyValue, type Tone } from './ui/Layout';
import { Button } from './ui/Button';
import { ConfirmDialog } from './ui/Dialog';

const STATE_TONE: Record<ServiceState, Tone> = {
  offline: 'danger',
  not_installed: 'warning',
  current: 'success',
  outdated: 'warning',
  newer: 'neutral',
};

/** Service install / upgrade / status (Windows: elevated, one click; Linux: instructions). */
export function ServiceBlock() {
  const { t, code } = useI18n();
  const status = useServiceStatus();
  const pushToast = useDaemon((s) => s.pushToast);
  const [confirm, setConfirm] = useState<'install' | 'upgrade' | null>(null);
  const [busy, setBusy] = useState(false);
  const s = status.data;

  const run = async (action: 'install' | 'upgrade') => {
    setBusy(true);
    try {
      const out = await app.serviceInstallOrUpgrade(action);
      if (out.status === 'completed') pushToast('success', t('service.done'));
      else if (out.status === 'cancelled') pushToast('info', t('service.cancelled'));
      else if (out.status === 'manual') pushToast('info', t('service.manual.toast'));
      else
        pushToast(
          'error',
          t('service.failed'),
          out.exit_code === null ? undefined : `exit ${out.exit_code}`,
        );
      await app.reconnect();
    } catch (e) {
      pushToast('error', describeError(code, e));
    } finally {
      setBusy(false);
      setConfirm(null);
      status.reload();
    }
  };

  const enableAutostart = async () => {
    try {
      const out = await app.agentInstallAutostart();
      if (out.ok) pushToast('success', t('agent.autostart.done'));
      else
        pushToast(
          'error',
          t('agent.autostart.failed'),
          out.exit_code === null ? undefined : `exit ${out.exit_code}`,
        );
    } catch (e) {
      pushToast('error', describeError(code, e));
    }
  };

  if (status.error && !s) {
    return (
      <Banner tone="danger" role="alert" title={t('service.status.error')}>
        {describeError(code, status.error)}
      </Banner>
    );
  }

  const canInstall = s && s.platform === 'windows' && s.installer_available;
  const needsInstall = s && (s.state === 'offline' || s.state === 'not_installed');
  const needsUpgrade = s?.state === 'outdated';

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        {s ? (
          <Badge tone={STATE_TONE[s.state]}>{t(`service.state.${s.state}`)}</Badge>
        ) : (
          <Badge>{t('common.loading')}</Badge>
        )}
        {s && s.agent_connected !== null ? (
          <Badge tone={s.agent_connected ? 'success' : 'warning'}>
            {s.agent_connected ? t('agent.connected') : t('agent.disconnected')}
          </Badge>
        ) : null}
      </div>

      {s ? (
        <dl className="divide-y divide-line">
          <KeyValue label={t('service.version.app')}>{s.app_version}</KeyValue>
          <KeyValue label={t('service.version.daemon')}>
            {s.daemon_version ?? t('common.unknown')}
          </KeyValue>
        </dl>
      ) : null}

      {s?.state === 'outdated' ? (
        <Banner tone="warning">{t('service.outdated.hint')}</Banner>
      ) : null}
      {s?.state === 'not_installed' ? (
        <Banner tone="neutral">{t('service.not_installed.hint')}</Banner>
      ) : null}
      {s?.state === 'offline' ? <Banner tone="neutral">{t('service.offline.hint')}</Banner> : null}
      {s && s.platform !== 'windows' && (needsInstall || needsUpgrade) ? (
        <Banner tone="neutral" title={t('service.manual.title')}>
          <code className="font-mono text-xs">{t('service.manual.command')}</code>
        </Banner>
      ) : null}

      <div className="flex flex-wrap gap-2">
        {canInstall && needsInstall ? (
          <Button
            variant="primary"
            icon={<Download className="size-4" aria-hidden />}
            onClick={() => setConfirm('install')}
          >
            {t('service.install')}
          </Button>
        ) : null}
        {canInstall && needsUpgrade ? (
          <Button
            variant="primary"
            icon={<Wrench className="size-4" aria-hidden />}
            onClick={() => setConfirm('upgrade')}
          >
            {t('service.upgrade')}
          </Button>
        ) : null}
        {s &&
        s.platform === 'windows' &&
        !s.installer_available &&
        (needsInstall || needsUpgrade) ? (
          <p className="text-xs text-muted">{t('service.binary_missing')}</p>
        ) : null}
        <Button
          icon={<RefreshCw className="size-4" aria-hidden />}
          onClick={status.reload}
          loading={status.loading && !s}
        >
          {t('common.refresh')}
        </Button>
        <Button
          onClick={() => void enableAutostart()}
          disabled={s ? !s.agent_binary_available : false}
        >
          {t('agent.autostart.enable')}
        </Button>
      </div>
      <p className="text-xs text-subtle">{t('agent.autostart.hint')}</p>

      <ConfirmDialog
        open={confirm !== null}
        title={
          confirm === 'upgrade'
            ? t('service.upgrade.confirm.title')
            : t('service.install.confirm.title')
        }
        description={t('service.confirm.body')}
        confirmLabel={confirm === 'upgrade' ? t('service.upgrade') : t('service.install')}
        cancelLabel={t('common.cancel')}
        loading={busy}
        onCancel={() => setConfirm(null)}
        onConfirm={() => confirm && void run(confirm)}
      />
    </div>
  );
}
