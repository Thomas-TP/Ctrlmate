import { useState } from 'react';
import { Copy, KeyRound } from 'lucide-react';
import { daemon } from '../../api/daemon';
import { useNow } from '../../hooks/hooks';
import { useI18n } from '../../i18n';
import { copyText } from '../../lib/clipboard';
import { formatCountdown, groupFingerprint } from '../../lib/format';
import { useActions } from '../../store/actions';
import { useDaemon } from '../../store/daemon';
import type { DaemonSnapshot } from '../../types/protocol';
import { Banner, Button } from '../../ui/kit';

export function Fingerprint({ hex }: { hex: string }) {
  const { t } = useI18n();
  const pushToast = useDaemon((s) => s.pushToast);
  const groups = groupFingerprint(hex);
  if (groups.length === 0) return null;
  const copy = async () => {
    const ok = await copyText(groups.join(''));
    pushToast(ok ? 'success' : 'error', ok ? t('common.copied') : t('common.copy_failed'));
  };
  return (
    <div className="rounded-xl bg-surface-2 p-4">
      <div className="flex items-center justify-between gap-2">
        <h4 className="text-xs font-medium text-muted">{t('ha.fingerprint.title')}</h4>
        <Button
          size="sm"
          variant="ghost"
          icon={<Copy className="size-3.5" aria-hidden />}
          onClick={() => void copy()}
        >
          {t('common.copy')}
        </Button>
      </div>
      <p
        className="mt-2 grid grid-cols-4 gap-x-3 gap-y-1 font-mono text-[12px] text-fg"
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

/** One-time PIN pairing: start, show the code with its countdown, cancel. */
export function PairingPanel({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const p = snap.pairing;
  const now = useNow(1000, p.active);
  const [busy, setBusy] = useState(false);
  const remaining = p.expires_at ? new Date(p.expires_at).getTime() - now : 0;
  const live = p.active && p.pin !== null && remaining > 0;

  const run = async (fn: () => Promise<DaemonSnapshot>) => {
    setBusy(true);
    await actions.snapshotCall(fn);
    setBusy(false);
  };
  const pin = p.pin ?? '';
  const grouped = pin.length === 6 ? `${pin.slice(0, 3)} ${pin.slice(3)}` : pin;

  return (
    <div className="space-y-4">
      {live ? (
        <div className="rounded-2xl border border-accent/30 bg-accent-soft p-5 text-center">
          <p className="text-xs font-medium text-accent-text">{t('ha.pairing.pin')}</p>
          <output
            aria-label={t('ha.pairing.pin.aria', { pin: pin.split('').join(' ') })}
            className="mt-1 block font-mono text-5xl font-semibold tracking-[0.14em] text-fg tabular"
          >
            {grouped}
          </output>
          <p
            className="mt-2 text-[13px] text-muted tabular"
            aria-label={t('ha.pairing.countdown.aria', { time: formatCountdown(remaining) })}
          >
            {t('ha.pairing.expires')} {formatCountdown(remaining)} ·{' '}
            {t('ha.pairing.port', { port: p.listen_port })}
          </p>
          <Button
            size="sm"
            variant="ghost"
            className="mt-2"
            loading={busy}
            onClick={() => void run(() => daemon.cancelPairing())}
          >
            {t('ha.pairing.cancel')}
          </Button>
        </div>
      ) : (
        <div className="space-y-3">
          {p.active ? (
            <Banner tone="warning">{t('ha.pairing.expired')}</Banner>
          ) : (
            <p className="text-[13px] text-muted">{t('ha.pairing.idle')}</p>
          )}
          <Button
            variant="primary"
            icon={<KeyRound className="size-4" aria-hidden />}
            loading={busy}
            onClick={() => void run(() => daemon.startPairing())}
          >
            {t('ha.pairing.start')}
          </Button>
        </div>
      )}
      <Fingerprint hex={p.cert_fingerprint} />
    </div>
  );
}
