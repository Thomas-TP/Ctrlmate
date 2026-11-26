import { useEffect, useState } from 'react';
import { Clipboard, Download, RefreshCw } from 'lucide-react';
import { app, listenUpdateProgress, type AppSettings, type AppUpdateStatus } from '../api/app';
import { ConnectionBanner } from '../components/ConnectionState';
import { ServiceBlock } from '../components/ServiceBlock';
import { Button } from '../components/ui/Button';
import { ConfirmDialog } from '../components/ui/Dialog';
import { Segmented, SwitchRow } from '../components/ui/Form';
import { Badge, Banner, Card, KeyValue, PageHeader } from '../components/ui/Layout';
import { useAppInfo } from '../hooks/hooks';
import { useI18n } from '../i18n';
import type { LanguagePref } from '../i18n/core';
import { copyText } from '../lib/clipboard';
import { buildDiagnostics } from '../lib/diagnostics';
import { describeError, useActions } from '../store/actions';
import { useDaemon } from '../store/daemon';
import { usePrefs, type ThemePref } from '../store/prefs';
import type { DaemonSnapshot, TemperatureUnit } from '../types/protocol';

export const REPO_URL = 'https://github.com/Thomas-TP/Ctrlmate';

function AppearanceCard({ snapshot }: { snapshot: DaemonSnapshot | null }) {
  const { t } = useI18n();
  const actions = useActions();
  const language = usePrefs((s) => s.language);
  const theme = usePrefs((s) => s.theme);
  const setLanguage = usePrefs((s) => s.setLanguage);
  const setTheme = usePrefs((s) => s.setTheme);
  const unit = snapshot?.temperature_unit ?? null;
  return (
    <Card title={t('settings.appearance.title')}>
      <div className="space-y-5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <p className="text-sm font-medium text-fg">{t('settings.language')}</p>
            <p className="text-xs text-muted">{t('settings.language.desc')}</p>
          </div>
          <Segmented<LanguagePref>
            label={t('settings.language')}
            value={language}
            onChange={setLanguage}
            options={[
              { value: 'system', label: t('settings.system') },
              { value: 'fr', label: 'Français' },
              { value: 'en', label: 'English' },
            ]}
          />
        </div>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <p className="text-sm font-medium text-fg">{t('settings.theme')}</p>
            <p className="text-xs text-muted">{t('settings.theme.desc')}</p>
          </div>
          <Segmented<ThemePref>
            label={t('settings.theme')}
            value={theme}
            onChange={setTheme}
            options={[
              { value: 'system', label: t('settings.system') },
              { value: 'light', label: t('settings.theme.light') },
              { value: 'dark', label: t('settings.theme.dark') },
            ]}
          />
        </div>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <p className="text-sm font-medium text-fg">{t('settings.unit')}</p>
            <p className="text-xs text-muted">
              {unit ? t('settings.unit.desc') : t('settings.unit.needs_daemon')}
            </p>
          </div>
          <Segmented<TemperatureUnit>
            label={t('settings.unit')}
            value={unit ?? 'celsius'}
            disabled={unit === null}
            onChange={(u) => void actions.settings({ temperature_unit: u })}
            options={[
              { value: 'celsius', label: '°C' },
              { value: 'fahrenheit', label: '°F' },
            ]}
          />
        </div>
      </div>
    </Card>
  );
}

function BehaviourCard() {
  const { t, code } = useI18n();
  const pushToast = useDaemon((s) => s.pushToast);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  useEffect(() => {
    app.settings().then(setSettings, () => setSettings(null));
  }, []);
  const update = async (patch: Partial<AppSettings>) => {
    if (!settings) return;
    const next = { ...settings, ...patch };
    try {
      await app.setSettings(next);
      setSettings(next);
    } catch (e) {
      pushToast('error', describeError(code, e));
    }
  };
  return (
    <Card title={t('settings.behaviour.title')}>
      {settings === null ? (
        <p className="text-sm text-muted">{t('settings.behaviour.unavailable')}</p>
      ) : (
        <div className="divide-y divide-line">
          <SwitchRow
            label={t('settings.launch_at_login')}
            description={t('settings.launch_at_login.desc')}
            checked={settings.launch_at_login}
            onChange={(launch_at_login) => void update({ launch_at_login })}
          />
          <SwitchRow
            label={t('settings.close_to_tray')}
            description={t('settings.close_to_tray.desc')}
            checked={settings.close_to_tray}
            onChange={(close_to_tray) => void update({ close_to_tray })}
          />
          <SwitchRow
            label={t('settings.start_hidden')}
            description={t('settings.start_hidden.desc')}
            checked={settings.start_hidden}
            onChange={(start_hidden) => void update({ start_hidden })}
          />
        </div>
      )}
    </Card>
  );
}

function HardwareCard({ snapshot }: { snapshot: DaemonSnapshot | null }) {
  const { t } = useI18n();
  const actions = useActions();
  const info = useAppInfo();
  if (!snapshot || info.data?.platform === 'linux') return null;
  const deep = snapshot.manifest.host.security_mode === 'deep_hardware_pawn_io';
  return (
    <Card title={t('settings.hardware.title')}>
      <SwitchRow
        label={t('settings.deep.title')}
        description={t('settings.deep.desc')}
        tone="warning"
        checked={deep}
        onChange={(v) =>
          void actions.settings({ security_mode: v ? 'deep_hardware_pawn_io' : 'zero_kernel_safe' })
        }
      />
      <dl className="mt-2 divide-y divide-line">
        <KeyValue label={t('settings.deep.bridge')}>
          <Badge tone={snapshot.deep_bridge_active ? 'success' : 'neutral'}>
            {snapshot.deep_bridge_active
              ? t('settings.deep.bridge.active')
              : t('settings.deep.bridge.inactive')}
          </Badge>
        </KeyValue>
      </dl>
    </Card>
  );
}

function UpdateRow() {
  const { t, code } = useI18n();
  const pushToast = useDaemon((s) => s.pushToast);
  const [status, setStatus] = useState<AppUpdateStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);

  useEffect(() => {
    let off: (() => void) | undefined;
    let disposed = false;
    void listenUpdateProgress((p) => setProgress(p)).then((u) => {
      if (disposed) u();
      else off = u;
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, []);

  const check = async () => {
    setChecking(true);
    try {
      setStatus(await app.updateCheck());
    } catch (e) {
      pushToast('error', describeError(code, e));
    } finally {
      setChecking(false);
    }
  };
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
      <div className="flex flex-wrap items-center gap-2">
        <Button
          icon={<RefreshCw className="size-4" aria-hidden />}
          loading={checking}
          onClick={() => void check()}
        >
          {t('settings.update.check')}
        </Button>
        {status?.state === 'available' ? (
          <Button
            variant="primary"
            icon={<Download className="size-4" aria-hidden />}
            onClick={() => setConfirm(true)}
            disabled={progress !== null}
          >
            {t('settings.update.install', { version: status.latest_version ?? '' })}
          </Button>
        ) : null}
      </div>
      <div aria-live="polite">
        {status?.state === 'up_to_date' ? (
          <Badge tone="success">{t('settings.update.up_to_date')}</Badge>
        ) : null}
        {status?.state === 'unsupported' ? (
          <Banner tone="neutral">{t('settings.update.unsupported')}</Banner>
        ) : null}
        {status?.state === 'error' ? (
          <Banner tone="warning" title={t('settings.update.error')}>
            {status.detail ? <span className="font-mono text-xs">{status.detail}</span> : null}
          </Banner>
        ) : null}
        {status?.state === 'available' ? (
          <Badge tone="accent">
            {t('settings.update.available', { version: status.latest_version ?? '' })}
          </Badge>
        ) : null}
      </div>
      {progress !== null ? (
        <div>
          <div
            role="progressbar"
            aria-label={t('settings.update.progress')}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.max(0, progress)}
            className="h-1.5 overflow-hidden rounded-full bg-surface-2"
          >
            <div
              className="h-full rounded-full bg-accent transition-all"
              style={{ width: `${Math.max(0, Math.min(100, progress))}%` }}
            />
          </div>
          <p className="mt-1 text-xs text-muted tabular-nums">{progress}%</p>
        </div>
      ) : null}
      <ConfirmDialog
        open={confirm}
        title={t('settings.update.confirm.title', { version: status?.latest_version ?? '' })}
        description={t('settings.update.confirm.body')}
        confirmLabel={t('settings.update.install.short')}
        cancelLabel={t('common.cancel')}
        onCancel={() => setConfirm(false)}
        onConfirm={() => void install()}
      />
    </div>
  );
}

function AppCard() {
  const { t } = useI18n();
  const info = useAppInfo();
  const i = info.data;
  return (
    <Card title={t('settings.app.title')}>
      <dl className="divide-y divide-line">
        <KeyValue label={t('settings.app.version')}>{i?.version ?? t('common.loading')}</KeyValue>
        <KeyValue label={t('settings.app.platform')}>
          {i ? `${i.platform} · ${i.arch}` : t('common.loading')}
        </KeyValue>
        <KeyValue label={t('settings.app.channel')}>
          {i?.update_channel ?? t('common.loading')}
        </KeyValue>
      </dl>
      <div className="mt-4">
        <UpdateRow />
      </div>
    </Card>
  );
}

function DiagnosticsCard({ snapshot }: { snapshot: DaemonSnapshot | null }) {
  const { t, locale } = useI18n();
  const info = useAppInfo();
  const connection = useDaemon((s) => s.connection);
  const pushToast = useDaemon((s) => s.pushToast);
  const copy = async () => {
    const text = buildDiagnostics({ app: info.data, connection, snapshot, locale });
    const ok = await copyText(text);
    pushToast(
      ok ? 'success' : 'error',
      ok ? t('settings.diagnostics.copied') : t('common.copy_failed'),
    );
  };
  return (
    <Card
      title={t('settings.diagnostics.title')}
      description={t('settings.diagnostics.description')}
    >
      {snapshot ? (
        <dl className="mb-4 grid gap-x-8 sm:grid-cols-2">
          <KeyValue label={t('settings.diagnostics.entities')}>
            {snapshot.manifest.entities.length}
          </KeyValue>
          <KeyValue label={t('settings.diagnostics.states')}>{snapshot.states.length}</KeyValue>
          <KeyValue label={t('settings.diagnostics.hid')}>{snapshot.hid_devices.length}</KeyValue>
          <KeyValue label={t('settings.diagnostics.backends')}>
            {snapshot.rgb_backends.length}
          </KeyValue>
          <KeyValue label={t('settings.diagnostics.protocol')}>
            {snapshot.manifest.protocol_version}
          </KeyValue>
          <KeyValue label={t('settings.diagnostics.token')}>
            {info.data?.token_source ?? t('common.unknown')}
          </KeyValue>
        </dl>
      ) : null}
      <Button icon={<Clipboard className="size-4" aria-hidden />} onClick={() => void copy()}>
        {t('settings.diagnostics.copy')}
      </Button>
      <p className="mt-2 text-xs text-subtle">{t('settings.diagnostics.privacy')}</p>
    </Card>
  );
}

function AboutCard() {
  const { t } = useI18n();
  return (
    <Card title={t('settings.about.title')}>
      <p className="text-sm text-muted">{t('settings.about.body')}</p>
      <dl className="mt-3 divide-y divide-line">
        <KeyValue label={t('settings.about.license')}>AGPL-3.0-or-later</KeyValue>
        <KeyValue label={t('settings.about.source')}>
          <span className="font-mono text-xs">{REPO_URL}</span>
        </KeyValue>
      </dl>
    </Card>
  );
}

export function Settings({ snapshot }: { snapshot: DaemonSnapshot | null }) {
  const { t } = useI18n();
  return (
    <>
      <PageHeader title={t('settings.title')} />
      {snapshot === null ? <ConnectionBanner /> : null}
      <div className="space-y-4">
        <AppearanceCard snapshot={snapshot} />
        <BehaviourCard />
        <HardwareCard snapshot={snapshot} />
        <Card title={t('settings.service.title')} description={t('settings.service.description')}>
          <ServiceBlock />
        </Card>
        <AppCard />
        <DiagnosticsCard snapshot={snapshot} />
        <AboutCard />
      </div>
    </>
  );
}
