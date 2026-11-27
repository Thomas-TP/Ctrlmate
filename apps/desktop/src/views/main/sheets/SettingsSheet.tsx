import { useState } from 'react';
import { ClipboardCopy, RefreshCw, RotateCcw } from 'lucide-react';
import { app, type AppSettings } from '../../../api/app';
import { daemon } from '../../../api/daemon';
import { useAppInfo, useResource } from '../../../hooks/hooks';
import { useI18n } from '../../../i18n';
import type { LanguagePref } from '../../../i18n/core';
import { copyText } from '../../../lib/clipboard';
import { buildDiagnostics } from '../../../lib/diagnostics';
import { useActions } from '../../../store/actions';
import { useDaemon } from '../../../store/daemon';
import { usePrefs, type ThemePref } from '../../../store/prefs';
import { SETTINGS_TABS, useUi, type SettingsTab } from '../../../store/ui';
import {
  POLLING_PROFILES,
  type CommandResult,
  type DaemonSnapshot,
  type PollingProfile,
  type TemperatureUnit,
} from '../../../types/protocol';
import {
  Badge,
  Banner,
  Button,
  KeyValue,
  Row,
  SectionTitle,
  Segmented,
  SwitchRow,
  cn,
} from '../../../ui/kit';
import { ServicePanel } from '../../shared/ServicePanel';
import { ResultNote } from './SystemSheet';
import { ScriptsEditor } from './ScriptsEditor';

export const REPO_URL = 'https://github.com/Thomas-TP/Ctrlmate';

/** Window behaviour lives in the Rust side (needed before the webview loads). */
export function useAppSettings() {
  const res = useResource(app.settings);
  const { data } = res;
  const [local, setLocal] = useState<AppSettings | null>(null);
  const [prev, setPrev] = useState(data);
  if (prev !== data) {
    setPrev(data);
    setLocal(null);
  }
  const value = local ?? data;
  const { t } = useI18n();
  const pushToast = useDaemon((s) => s.pushToast);
  const set = async (patch: Partial<AppSettings>) => {
    if (!value) return;
    const next = { ...value, ...patch };
    setLocal(next);
    try {
      await app.setSettings(next);
    } catch {
      setLocal(null);
      pushToast('error', t('settings.behaviour.unavailable'));
    }
  };
  return { value, set, loading: res.loading };
}

function General({ snap }: { snap: DaemonSnapshot | null }) {
  const { t } = useI18n();
  const actions = useActions();
  const language = usePrefs((s) => s.language);
  const theme = usePrefs((s) => s.theme);
  const { setLanguage, setTheme, setOnboarded } = usePrefs.getState();
  const closeSheet = useUi((s) => s.closeSheet);
  const win = useAppSettings();
  return (
    <div>
      <SectionTitle>{t('settings.appearance.title')}</SectionTitle>
      <Row label={t('settings.language')} description={t('settings.language.desc')}>
        <Segmented<LanguagePref>
          size="sm"
          label={t('settings.language')}
          value={language}
          onChange={setLanguage}
          options={[
            { value: 'system', label: t('settings.system') },
            { value: 'fr', label: 'Français' },
            { value: 'en', label: 'English' },
          ]}
        />
      </Row>
      <Row label={t('settings.theme')} description={t('settings.theme.desc')}>
        <Segmented<ThemePref>
          size="sm"
          label={t('settings.theme')}
          value={theme}
          onChange={setTheme}
          options={[
            { value: 'system', label: t('settings.system') },
            { value: 'light', label: t('settings.theme.light') },
            { value: 'dark', label: t('settings.theme.dark') },
          ]}
        />
      </Row>
      <Row
        label={t('settings.unit')}
        description={snap ? t('settings.unit.desc') : t('settings.unit.needs_daemon')}
      >
        <Segmented<TemperatureUnit>
          size="sm"
          label={t('settings.unit')}
          value={snap?.temperature_unit ?? null}
          disabled={!snap}
          onChange={(u) => void actions.settings({ temperature_unit: u })}
          options={[
            { value: 'celsius', label: '°C' },
            { value: 'fahrenheit', label: '°F' },
          ]}
        />
      </Row>

      <SectionTitle>{t('settings.behaviour.title')}</SectionTitle>
      {win.value ? (
        <div className="divide-y divide-line">
          <SwitchRow
            label={t('settings.launch_at_login')}
            description={t('settings.launch_at_login.desc')}
            checked={win.value.launch_at_login}
            onChange={(v) => void win.set({ launch_at_login: v })}
          />
          <SwitchRow
            label={t('settings.close_to_tray')}
            description={t('settings.close_to_tray.desc')}
            checked={win.value.close_to_tray}
            onChange={(v) => void win.set({ close_to_tray: v })}
          />
          <SwitchRow
            label={t('settings.start_hidden')}
            description={t('settings.start_hidden.desc')}
            checked={win.value.start_hidden}
            onChange={(v) => void win.set({ start_hidden: v })}
          />
        </div>
      ) : (
        <p className="text-[13px] text-muted">{t('settings.behaviour.unavailable')}</p>
      )}

      <SectionTitle>{t('onb.replay.title')}</SectionTitle>
      <Button
        icon={<RotateCcw className="size-4" aria-hidden />}
        onClick={() => {
          setOnboarded(false);
          closeSheet();
        }}
      >
        {t('onb.replay')}
      </Button>
    </div>
  );
}

function Hardware({ snap }: { snap: DaemonSnapshot | null }) {
  const { t, code } = useI18n();
  const actions = useActions();
  const [busy, setBusy] = useState(false);
  return (
    <div>
      <SectionTitle>{t('settings.service.title')}</SectionTitle>
      <ServicePanel />
      {snap ? (
        <>
          <SectionTitle>{t('settings.hardware.title')}</SectionTitle>
          <SwitchRow
            label={t('settings.deep.title')}
            description={t('settings.deep.desc')}
            checked={snap.manifest.host.security_mode === 'deep_hardware_pawn_io'}
            onChange={(on) =>
              void actions.settings({
                security_mode: on ? 'deep_hardware_pawn_io' : 'zero_kernel_safe',
              })
            }
          />
          <p className="text-[13px] text-muted">
            {t('settings.deep.bridge')}:{' '}
            <span className="font-medium text-fg">
              {snap.deep_bridge_active
                ? t('settings.deep.bridge.active')
                : t('settings.deep.bridge.inactive')}
            </span>
          </p>

          <SectionTitle>{t('hw.conflicts.title')}</SectionTitle>
          <ul className="divide-y divide-line rounded-xl border border-line px-4 text-sm">
            {snap.conflicts.length === 0 ? (
              <li className="py-3 text-muted">{t('hw.conflicts.none')}</li>
            ) : (
              snap.conflicts.map((c) => (
                <li key={c.id} className="flex items-center justify-between gap-3 py-3">
                  <span className="font-medium text-fg">{c.name}</span>
                  <Badge tone={c.running ? 'warning' : 'neutral'} dot>
                    {c.running ? t('hw.conflicts.running') : t('hw.conflicts.idle')}
                  </Badge>
                </li>
              ))
            )}
          </ul>
          {snap.conflicts.some((c) => c.running) ? (
            <p className="mt-2 text-[13px] text-muted">{t('hw.conflicts.hint')}</p>
          ) : null}

          <SectionTitle>{t('hw.devices.title')}</SectionTitle>
          <ul className="divide-y divide-line rounded-xl border border-line px-4 text-sm">
            {snap.manifest.child_devices.map((d) => (
              <li key={d.id} className="py-2.5">
                <p className="font-medium text-fg">{d.name}</p>
                <p className="text-xs text-muted">
                  {[d.manufacturer, d.model].filter(Boolean).join(' · ')}
                </p>
              </li>
            ))}
            {snap.rgb_devices.map((d) => (
              <li key={d.id} className="flex items-center justify-between gap-3 py-2.5">
                <span className="text-fg">{d.name}</span>
                <span className="text-xs text-muted">{code('rgb.reason', d.reason)}</span>
              </li>
            ))}
          </ul>
          <div className="mt-3">
            <Button
              icon={<RefreshCw className="size-4" aria-hidden />}
              loading={busy}
              onClick={() =>
                void (async () => {
                  setBusy(true);
                  await actions.snapshotCall(() => daemon.rescanHardware(), {
                    successKey: 'lighting.rescan.done',
                  });
                  setBusy(false);
                })()
              }
            >
              {t('lighting.rescan')}
            </Button>
          </div>
        </>
      ) : null}
    </div>
  );
}

function Advanced({ snap }: { snap: DaemonSnapshot | null }) {
  const { t } = useI18n();
  const actions = useActions();
  const [last, setLast] = useState<{ label: string; result: CommandResult | null } | null>(null);
  if (!snap) return <Banner>{t('settings.unit.needs_daemon')}</Banner>;
  return (
    <div>
      <SectionTitle>{t('ha.polling.title')}</SectionTitle>
      <p className="mb-3 text-[13px] text-muted">{t('ha.polling.description')}</p>
      <div role="radiogroup" aria-label={t('ha.polling.title')} className="space-y-2">
        {POLLING_PROFILES.map((p: PollingProfile) => (
          <button
            key={p}
            type="button"
            role="radio"
            aria-checked={snap.polling_profile === p}
            onClick={() => void actions.settings({ polling_profile: p })}
            className={cn(
              'block w-full rounded-xl border px-4 py-3 text-left transition-colors',
              snap.polling_profile === p
                ? 'border-accent bg-accent-soft'
                : 'border-line hover:bg-surface-2',
            )}
          >
            <span className="block text-sm font-medium text-fg">{t(`polling.${p}`)}</span>
            <span className="block text-[13px] text-muted">{t(`polling.${p}.desc`)}</span>
          </button>
        ))}
      </div>
      <SectionTitle>{t('scripts.title')}</SectionTitle>
      <p className="mb-3 text-[13px] text-muted">{t('scripts.description')}</p>
      <ScriptsEditor snap={snap} onResult={(label, result) => setLast({ label, result })} />
      <ResultNote last={last} />
    </div>
  );
}

function About({ snap }: { snap: DaemonSnapshot | null }) {
  const { t, locale } = useI18n();
  const info = useAppInfo().data;
  const connection = useDaemon((s) => s.connection);
  const pushToast = useDaemon((s) => s.pushToast);
  const i = info;
  const copy = async () => {
    const text = buildDiagnostics({ app: info, connection, snapshot: snap, locale });
    const ok = await copyText(text);
    pushToast(
      ok ? 'success' : 'error',
      ok ? t('settings.diagnostics.copied') : t('common.copy_failed'),
    );
  };
  return (
    <div>
      <SectionTitle>{t('settings.app.title')}</SectionTitle>
      <dl className="divide-y divide-line">
        <KeyValue label={t('settings.app.version')}>{i?.version ?? t('common.loading')}</KeyValue>
        <KeyValue label={t('settings.app.platform')}>
          {i ? `${i.platform} · ${i.arch}` : t('common.loading')}
        </KeyValue>
        <KeyValue label={t('settings.app.channel')}>
          {i?.update_channel ?? t('common.loading')}
        </KeyValue>
        <KeyValue label={t('settings.about.license')}>AGPL-3.0-or-later</KeyValue>
      </dl>
      <p className="mt-3 text-[13px] text-muted">{t('settings.about.body')}</p>
      <p className="mt-1 text-[13px]">
        <a
          className="font-medium text-accent-text underline-offset-2 hover:underline"
          href={REPO_URL}
          target="_blank"
          rel="noreferrer"
        >
          {t('settings.about.source')}
        </a>
      </p>
      <SectionTitle>{t('settings.diagnostics.title')}</SectionTitle>
      <p className="mb-3 text-[13px] text-muted">{t('settings.diagnostics.description')}</p>
      <Button icon={<ClipboardCopy className="size-4" aria-hidden />} onClick={() => void copy()}>
        {t('settings.diagnostics.copy')}
      </Button>
      <p className="mt-2 text-xs text-subtle">{t('settings.diagnostics.privacy')}</p>
    </div>
  );
}

export function SettingsSheet({ snap }: { snap: DaemonSnapshot | null }) {
  const { t } = useI18n();
  const tab = useUi((s) => s.settingsTab);
  const setTab = useUi((s) => s.setSettingsTab);
  return (
    <div>
      <div className="mb-5">
        <Segmented<SettingsTab>
          full
          label={t('settings.title')}
          value={tab}
          onChange={setTab}
          options={SETTINGS_TABS.map((id) => ({ value: id, label: t(`settings.tab.${id}`) }))}
        />
      </div>
      {tab === 'general' ? <General snap={snap} /> : null}
      {tab === 'hardware' ? <Hardware snap={snap} /> : null}
      {tab === 'advanced' ? <Advanced snap={snap} /> : null}
      {tab === 'about' ? <About snap={snap} /> : null}
    </div>
  );
}
