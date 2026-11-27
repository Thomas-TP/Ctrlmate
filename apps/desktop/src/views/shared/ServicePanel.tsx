import { useState } from 'react';
import { Download, RefreshCw, Wrench } from 'lucide-react';
import { app, type ServiceState } from '../../api/app';
import { useServiceSetup } from '../../hooks/useServiceSetup';
import { useI18n } from '../../i18n';
import { useDaemon } from '../../store/daemon';
import { Badge, Banner, Button, KeyValue, type Tone } from '../../ui/kit';
import { ConfirmDialog } from '../../ui/overlay';

const TONE: Record<ServiceState, Tone> = {
  offline: 'danger',
  not_installed: 'warning',
  current: 'success',
  outdated: 'warning',
  newer: 'neutral',
};

/** Service + agent status with the one-click elevated install / upgrade. */
export function ServicePanel() {
  const { t } = useI18n();
  const { status, phase, run } = useServiceSetup();
  const pushToast = useDaemon((s) => s.pushToast);
  const [confirm, setConfirm] = useState<'install' | 'upgrade' | null>(null);
  const s = status.data;

  const exec = async (action: 'install' | 'upgrade') => {
    setConfirm(null);
    const out = await run(action);
    if (out === 'done') pushToast('success', t('service.done'));
    else if (out === 'cancelled') pushToast('info', t('service.cancelled'));
    else if (out === 'manual') pushToast('info', t('service.manual.toast'));
    else pushToast('error', t('service.failed'));
  };

  const windows = s?.platform === 'windows';
  const needsInstall = s?.state === 'offline' || s?.state === 'not_installed';
  const needsUpgrade = s?.state === 'outdated';

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        {s ? (
          <Badge tone={TONE[s.state]} dot>
            {t(`service.state.${s.state}`)}
          </Badge>
        ) : (
          <Badge>{t('common.loading')}</Badge>
        )}
        {s && s.agent_connected !== null ? (
          <Badge tone={s.agent_connected ? 'success' : 'warning'} dot>
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
      {s?.state === 'not_installed' ? <Banner>{t('service.not_installed.hint')}</Banner> : null}
      {s?.state === 'offline' ? <Banner>{t('service.offline.hint')}</Banner> : null}
      {s && !windows && (needsInstall || needsUpgrade) ? (
        <Banner title={t('service.manual.title')}>
          <code className="font-mono text-xs">{t('service.manual.command')}</code>
        </Banner>
      ) : null}
      <div className="flex flex-wrap gap-2">
        {windows && s.installer_available && needsInstall ? (
          <Button
            variant="primary"
            icon={<Download className="size-4" aria-hidden />}
            loading={phase === 'working'}
            onClick={() => setConfirm('install')}
          >
            {t('service.install')}
          </Button>
        ) : null}
        {windows && s.installer_available && needsUpgrade ? (
          <Button
            variant="primary"
            icon={<Wrench className="size-4" aria-hidden />}
            loading={phase === 'working'}
            onClick={() => setConfirm('upgrade')}
          >
            {t('service.upgrade')}
          </Button>
        ) : null}
        {windows && !s.installer_available && (needsInstall || needsUpgrade) ? (
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
          disabled={s ? !s.agent_binary_available : false}
          onClick={() =>
            void app
              .agentInstallAutostart()
              .then((o) =>
                pushToast(
                  o.ok ? 'success' : 'error',
                  o.ok ? t('agent.autostart.done') : t('agent.autostart.failed'),
                ),
              )
          }
        >
          {t('agent.autostart.enable')}
        </Button>
      </div>
      <ConfirmDialog
        open={confirm !== null}
        title={
          confirm === 'upgrade'
            ? t('service.upgrade.confirm.title')
            : t('service.install.confirm.title')
        }
        description={t('service.confirm.body')}
        confirmLabel={confirm === 'upgrade' ? t('service.upgrade') : t('service.install')}
        loading={phase === 'working'}
        onCancel={() => setConfirm(null)}
        onConfirm={() => confirm && void exec(confirm)}
      />
    </div>
  );
}
