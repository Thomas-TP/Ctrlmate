import { Loader2, PlugZap, RefreshCw, ShieldAlert } from 'lucide-react';
import { app } from '../api/app';
import { useI18n } from '../i18n';
import { useDaemon } from '../store/daemon';
import { ServiceBlock } from './ServiceBlock';
import { Button } from './ui/Button';
import { Banner, Card, EmptyState } from './ui/Layout';

/** Help text per reason code, shown under the headline. */
function HelpFor({ reason }: { reason: string | null | undefined }) {
  const { t } = useI18n();
  switch (reason) {
    case 'token_permission_denied':
      return (
        <>
          <p>{t('connection.help.token_permission_denied')}</p>
          <p className="mt-2">
            <code className="font-mono text-xs">
              {t('connection.help.token_permission_denied.command')}
            </code>
          </p>
        </>
      );
    case 'token_not_found':
      return <p>{t('connection.help.token_not_found')}</p>;
    case 'token_invalid':
      return <p>{t('connection.help.token_invalid')}</p>;
    case 'unauthorized':
      return <p>{t('connection.help.unauthorized')}</p>;
    default:
      return <p>{t('connection.help.offline')}</p>;
  }
}

/** Full-page state when no snapshot has ever been received. Designed, never blank. */
export function ConnectionScreen() {
  const { t, code } = useI18n();
  const conn = useDaemon((s) => s.connection);

  if (conn.status === 'connecting') {
    return (
      <div className="flex h-full items-center justify-center p-8">
        <EmptyState
          icon={<Loader2 className="cm-spin size-6" aria-hidden />}
          title={t('connection.connecting.title')}
        >
          {t('connection.connecting.body')}
        </EmptyState>
      </div>
    );
  }

  const unauthorized = conn.status === 'unauthorized';
  const reason = conn.reason ?? (unauthorized ? 'unauthorized' : 'daemon_offline');
  return (
    <div className="mx-auto flex h-full max-w-xl flex-col justify-center gap-5 p-6">
      <EmptyState
        icon={
          unauthorized ? (
            <ShieldAlert className="size-7" aria-hidden />
          ) : (
            <PlugZap className="size-7" aria-hidden />
          )
        }
        title={unauthorized ? t('connection.unauthorized.title') : t('connection.offline.title')}
        actions={
          <Button
            variant="primary"
            icon={<RefreshCw className="size-4" aria-hidden />}
            onClick={() => void app.reconnect()}
          >
            {t('connection.retry')}
          </Button>
        }
      >
        <HelpFor reason={reason} />
        <p className="mt-3 text-xs text-subtle">
          {code('connection.reason', reason)}
          {conn.retry_in_ms
            ? ` · ${t('connection.retrying', { seconds: Math.ceil(conn.retry_in_ms / 1000) })}`
            : ''}
        </p>
      </EmptyState>
      <Card title={t('settings.service.title')}>
        <ServiceBlock />
      </Card>
    </div>
  );
}

/** Slim banner when we have stale data but the live connection is down. */
export function ConnectionBanner() {
  const { t, code } = useI18n();
  const conn = useDaemon((s) => s.connection);
  if (conn.status === 'connected') return null;
  const reason = conn.reason ?? 'daemon_offline';
  return (
    <div className="mb-4">
      <Banner
        tone="warning"
        role="status"
        title={
          conn.status === 'connecting' ? t('connection.reconnecting') : t('connection.stale.title')
        }
        actions={
          <Button size="sm" onClick={() => void app.reconnect()}>
            {t('connection.retry')}
          </Button>
        }
      >
        {t('connection.stale.body')}{' '}
        {conn.status === 'connecting' ? '' : code('connection.reason', reason)}
      </Banner>
    </div>
  );
}
