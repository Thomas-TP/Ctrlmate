import { useEffect } from 'react';
import { RefreshCw, Search, Settings, ShieldAlert } from 'lucide-react';
import { app } from '../../api/app';
import { EVENT_OPEN_SHEET, on } from '../../api/tauri';
import { useI18n } from '../../i18n';
import { useDaemon } from '../../store/daemon';
import { usePrefs } from '../../store/prefs';
import { isSheetId, useUi, type SheetId } from '../../store/ui';
import type { DaemonSnapshot } from '../../types/protocol';
import { Banner, Button, IconButton, Spinner } from '../../ui/kit';
import { Sheet, Toasts } from '../../ui/overlay';
import { Onboarding } from '../onboarding/Onboarding';
import { ServicePanel } from '../shared/ServicePanel';
import { Dashboard } from './Dashboard';
import { Palette } from './Palette';
import { CoolingSheet } from './sheets/CoolingSheet';
import { HaSheet } from './sheets/HaSheet';
import { LightingSheet } from './sheets/LightingSheet';
import { SettingsSheet } from './sheets/SettingsSheet';
import { SystemSheet } from './sheets/SystemSheet';
import { TempsSheet } from './sheets/TempsSheet';
import { UpdatesSheet } from './sheets/UpdatesSheet';

function Brand() {
  return (
    <div className="flex items-center gap-2.5">
      <span
        className="flex size-7 items-center justify-center rounded-lg bg-accent text-[13px] font-bold text-accent-fg"
        aria-hidden
      >
        C
      </span>
      <span className="text-[15px] font-semibold tracking-tight text-fg">Ctrlmate</span>
    </div>
  );
}

function TopBar() {
  const { t } = useI18n();
  const setPalette = useUi((s) => s.setPaletteOpen);
  const openSheet = useUi((s) => s.openSheet);
  return (
    <header className="flex h-14 shrink-0 items-center justify-between border-b border-line bg-surface/80 px-5 backdrop-blur">
      <Brand />
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          onClick={() => setPalette(true)}
          className="flex h-9 items-center gap-3 rounded-control border border-line bg-surface px-3 text-[13px] text-muted transition-colors hover:border-line-strong hover:text-fg"
        >
          <Search className="size-4" aria-hidden />
          <span className="hidden sm:inline">{t('palette.open')}</span>
          <kbd className="rounded-md border border-line bg-surface-2 px-1.5 py-0.5 font-sans text-[11px] text-subtle">
            Ctrl K
          </kbd>
        </button>
        <IconButton label={t('nav.settings')} onClick={() => openSheet('settings')}>
          <Settings className="size-[18px]" aria-hidden />
        </IconButton>
      </div>
    </header>
  );
}

/** Shown when there is no snapshot yet (and the onboarding is done): explain and offer the fix. */
function ConnectionScreen() {
  const { t, code } = useI18n();
  const conn = useDaemon((s) => s.connection);
  const reason = conn.reason ?? (conn.status === 'offline' ? 'daemon_offline' : null);
  if (conn.status === 'connecting') {
    return (
      <div className="flex flex-1 items-center justify-center">
        <div className="text-center">
          <Spinner label={t('connection.connecting.title')} />
          <p className="mt-2 text-[13px] text-muted">{t('connection.connecting.body')}</p>
        </div>
      </div>
    );
  }
  const help =
    reason === 'token_not_found'
      ? t('connection.help.token_not_found')
      : reason === 'token_invalid'
        ? t('connection.help.token_invalid')
        : reason === 'token_permission_denied'
          ? t('connection.help.token_permission_denied')
          : reason === 'unauthorized'
            ? t('connection.help.unauthorized')
            : t('connection.help.offline');
  const seconds = conn.retry_in_ms ? Math.max(1, Math.round(conn.retry_in_ms / 1000)) : null;
  return (
    <div className="mx-auto flex w-full max-w-lg flex-1 flex-col justify-center gap-5 px-6 py-10">
      <div className="text-center">
        <span className="mx-auto mb-4 flex size-12 items-center justify-center rounded-2xl bg-warning-soft text-warning">
          <ShieldAlert className="size-6" aria-hidden />
        </span>
        <h1 className="text-xl font-semibold tracking-tight text-fg">
          {conn.status === 'unauthorized'
            ? t('connection.unauthorized.title')
            : t('connection.offline.title')}
        </h1>
        <p className="mt-2 text-sm text-muted">{help}</p>
        {reason === 'token_permission_denied' ? (
          <code className="mt-2 block rounded-lg bg-surface-2 px-3 py-2 font-mono text-xs">
            {t('connection.help.token_permission_denied.command')}
          </code>
        ) : null}
        {reason ? (
          <p className="mt-2 text-xs text-subtle">
            {code('connection.reason', reason)}
            {seconds ? ` · ${t('connection.retrying', { seconds })}` : ''}
          </p>
        ) : null}
      </div>
      <div className="rounded-card border border-line bg-surface p-5">
        <ServicePanel />
      </div>
      <div className="flex justify-center">
        <Button
          icon={<RefreshCw className="size-4" aria-hidden />}
          onClick={() => void app.reconnect()}
        >
          {t('connection.retry')}
        </Button>
      </div>
    </div>
  );
}

function StaleBanner() {
  const { t, code } = useI18n();
  const conn = useDaemon((s) => s.connection);
  if (conn.status === 'connected') return null;
  return (
    <div className="mx-auto w-full max-w-[1120px] px-6 pt-4">
      <Banner
        tone="warning"
        role="alert"
        title={t('connection.stale.title')}
        action={
          <Button size="sm" onClick={() => void app.reconnect()}>
            {t('connection.retry')}
          </Button>
        }
      >
        {conn.reason ? code('connection.reason', conn.reason) : t('connection.reconnecting')}
      </Banner>
    </div>
  );
}

const SHEET_TITLE: Record<
  SheetId,
  | 'nav.cooling'
  | 'dash.temps.title'
  | 'nav.lighting'
  | 'nav.homeAssistant'
  | 'dash.pc.title'
  | 'dash.updates.title'
  | 'nav.settings'
> = {
  cooling: 'nav.cooling',
  temps: 'dash.temps.title',
  lighting: 'nav.lighting',
  ha: 'nav.homeAssistant',
  system: 'dash.pc.title',
  updates: 'dash.updates.title',
  settings: 'nav.settings',
};

function Sheets({ snap }: { snap: DaemonSnapshot | null }) {
  const { t } = useI18n();
  const sheet = useUi((s) => s.sheet);
  const close = useUi((s) => s.closeSheet);
  // Keep the last sheet mounted-by-id so the dialog can close cleanly.
  const id: SheetId = sheet ?? 'cooling';
  const needsSnap = id !== 'settings';
  return (
    <Sheet
      open={sheet !== null}
      onClose={close}
      title={t(SHEET_TITLE[id])}
      width={id === 'settings' ? 560 : 500}
    >
      {id === 'settings' ? <SettingsSheet snap={snap} /> : null}
      {needsSnap && !snap ? <Banner>{t('connection.reconnecting')}</Banner> : null}
      {snap && id === 'cooling' ? <CoolingSheet snap={snap} /> : null}
      {snap && id === 'temps' ? <TempsSheet snap={snap} /> : null}
      {snap && id === 'lighting' ? <LightingSheet snap={snap} /> : null}
      {snap && id === 'ha' ? <HaSheet snap={snap} /> : null}
      {snap && id === 'system' ? <SystemSheet snap={snap} /> : null}
      {snap && id === 'updates' ? <UpdatesSheet snap={snap} /> : null}
    </Sheet>
  );
}

export function MainView() {
  const snap = useDaemon((s) => s.snapshot);
  const onboarded = usePrefs((s) => s.onboarded);
  const openSheet = useUi((s) => s.openSheet);

  // The tray panel asks the main window to open a section.
  useEffect(() => {
    let off: (() => void) | undefined;
    let disposed = false;
    void on<string>(EVENT_OPEN_SHEET, (id) => {
      if (isSheetId(id)) openSheet(id);
    }).then((fn) => {
      if (disposed) fn();
      else off = fn;
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, [openSheet]);

  if (!onboarded) {
    return (
      <>
        <Onboarding />
        <Toasts />
      </>
    );
  }
  return (
    <div className="flex h-full flex-col">
      <TopBar />
      <main className="flex min-h-0 flex-1 flex-col overflow-y-auto">
        {snap ? (
          <>
            <StaleBanner />
            <Dashboard snap={snap} />
          </>
        ) : (
          <ConnectionScreen />
        )}
      </main>
      <Sheets snap={snap} />
      <Palette snap={snap} />
      <Toasts />
    </div>
  );
}
