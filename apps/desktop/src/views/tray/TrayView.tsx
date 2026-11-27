import { useEffect } from 'react';
import { ArrowUpRight, Settings } from 'lucide-react';
import { app } from '../../api/app';
import { useI18n } from '../../i18n';
import { numberOf } from '../../lib/snapshot';
import { hottest } from '../../lib/derive';
import { useDaemon } from '../../store/daemon';
import { ENTITY, type DaemonSnapshot } from '../../types/protocol';
import { Badge, Button, IconButton, Spinner } from '../../ui/kit';
import { Toasts } from '../../ui/overlay';
import { MasterLight, PresetChips, QuickActions } from '../shared/controls';

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 flex-1">
      <p className="text-[11px] font-medium text-subtle uppercase tracking-wide">{label}</p>
      <p className="mt-0.5 text-lg font-semibold text-fg tabular">{value}</p>
    </div>
  );
}

function Panel({ snap }: { snap: DaemonSnapshot }) {
  const { t, temp, pct } = useI18n();
  const failsafe = snap.cooling_policy?.state === 'failsafe' || snap.thermal_failsafe_active;
  const top = hottest(snap);
  return (
    <div className="space-y-5">
      <div>
        <div className="flex items-start justify-between gap-3">
          <div>
            <p className="text-[12px] font-medium text-subtle">{snap.manifest.host.name}</p>
            <p className="mt-0.5 text-[34px] leading-none font-semibold tracking-tight text-fg tabular">
              {temp(top)}
            </p>
          </div>
          <Badge tone={failsafe ? 'danger' : 'success'} dot>
            {failsafe ? t('cooling.state.failsafe') : t('tray.protected')}
          </Badge>
        </div>
        <div className="mt-4 flex gap-4">
          <Stat label={t('dash.short.cpu')} value={temp(numberOf(snap, ENTITY.cpuTemp))} />
          <Stat label={t('dash.short.gpu')} value={temp(numberOf(snap, ENTITY.gpuTemp))} />
          <Stat label={t('dash.short.liquid')} value={temp(numberOf(snap, ENTITY.liquidTemp))} />
          <Stat label={t('metric.pump')} value={pct(numberOf(snap, ENTITY.pumpDuty))} />
        </div>
      </div>

      <section aria-label={t('nav.cooling')}>
        <p className="mb-2 text-xs font-semibold text-subtle uppercase tracking-wide">
          {t('nav.cooling')}
        </p>
        <PresetChips snap={snap} />
      </section>

      <section aria-label={t('nav.lighting')}>
        <p className="mb-2 text-xs font-semibold text-subtle uppercase tracking-wide">
          {t('nav.lighting')}
        </p>
        <MasterLight snap={snap} compact />
      </section>

      <QuickActions snap={snap} />
    </div>
  );
}

export function TrayView() {
  const { t } = useI18n();
  const snap = useDaemon((s) => s.snapshot);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') void app.hideTray().catch(() => null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <div className="flex h-full flex-col overflow-hidden rounded-[14px] border border-line bg-surface">
      <div className="min-h-0 flex-1 overflow-y-auto p-5">
        {snap ? (
          <Panel snap={snap} />
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-center">
            <Spinner label={t('tray.connecting')} />
            <p className="text-[13px] text-muted">{t('tray.offline.body')}</p>
          </div>
        )}
      </div>
      <footer className="flex items-center justify-between border-t border-line px-4 py-2.5">
        <IconButton label={t('nav.settings')} onClick={() => void app.openMain('settings')}>
          <Settings className="size-[18px]" aria-hidden />
        </IconButton>
        <Button
          size="sm"
          variant="primary"
          icon={<ArrowUpRight className="size-4" aria-hidden />}
          onClick={() => void app.openMain()}
        >
          {t('tray.open')}
        </Button>
      </footer>
      <Toasts />
    </div>
  );
}
