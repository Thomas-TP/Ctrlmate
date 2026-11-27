import { useState, type ReactNode } from 'react';
import {
  ArrowLeft,
  ArrowRight,
  Check,
  Copy,
  Cpu,
  Fan,
  Home,
  Lightbulb,
  MonitorCog,
  RefreshCw,
  ShieldCheck,
  Sparkles,
  type LucideIcon,
} from 'lucide-react';
import { app } from '../../api/app';
import { daemon } from '../../api/daemon';
import { useServiceSetup } from '../../hooks/useServiceSetup';
import { useI18n } from '../../i18n';
import type { LanguagePref } from '../../i18n/core';
import { copyText } from '../../lib/clipboard';
import { hasEntity, numberOf, readSensor } from '../../lib/snapshot';
import { useActions } from '../../store/actions';
import { useDaemon } from '../../store/daemon';
import { usePrefs, type ThemePref } from '../../store/prefs';
import { ENTITY, type DaemonSnapshot, type TemperatureUnit } from '../../types/protocol';
import { Badge, Banner, Button, Row, Segmented, Spinner, SwitchRow, cn } from '../../ui/kit';
import { PairingPanel } from '../shared/Pairing';
import { PresetCards } from '../shared/controls';
import { useAppSettings, REPO_URL } from '../main/sheets/SettingsSheet';

const STEPS = ['welcome', 'hardware', 'cooling', 'ha', 'prefs'] as const;
type Step = (typeof STEPS)[number];

function Heading({
  icon: Icon,
  title,
  children,
}: {
  icon: LucideIcon;
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="cm-enter">
      <span className="mb-5 flex size-12 items-center justify-center rounded-2xl bg-accent-soft text-accent-text">
        <Icon className="size-6" aria-hidden />
      </span>
      <h1 className="text-[28px] leading-tight font-semibold tracking-tight text-fg">{title}</h1>
      {children ? <p className="mt-2 max-w-md text-[15px] text-muted">{children}</p> : null}
    </div>
  );
}

// --- Step 1: install --------------------------------------------------------------------------

function Welcome({ snap, onReady }: { snap: DaemonSnapshot | null; onReady: () => void }) {
  const { t } = useI18n();
  const { status, phase, run } = useServiceSetup();
  const s = status.data;
  const ready = snap !== null;
  const windows = s?.platform === 'windows';
  const items: [
    LucideIcon,
    'onb.install.service' | 'onb.install.agent' | 'onb.install.firewall',
  ][] = [
    [Cpu, 'onb.install.service'],
    [MonitorCog, 'onb.install.agent'],
    [ShieldCheck, 'onb.install.firewall'],
  ];
  return (
    <div className="space-y-6">
      <Heading icon={Sparkles} title={t('onb.welcome.title')}>
        {t('onb.welcome.body')}
      </Heading>
      <ul className="space-y-2.5">
        {items.map(([Icon, key]) => (
          <li
            key={key}
            className="flex items-start gap-3 rounded-xl border border-line bg-surface px-4 py-3"
          >
            <Icon className="mt-0.5 size-[18px] shrink-0 text-subtle" aria-hidden />
            <div>
              <p className="text-sm font-medium text-fg">{t(key)}</p>
              <p className="text-[13px] text-muted">{t(`${key}.desc`)}</p>
            </div>
          </li>
        ))}
      </ul>

      {ready ? (
        <Banner tone="success" title={t('onb.install.ready')} role="status">
          {t('onb.install.ready.body')}
        </Banner>
      ) : phase === 'working' ? (
        <div className="rounded-xl bg-surface-2 px-4 py-3">
          <Spinner label={t('onb.install.working')} />
          <p className="mt-1 text-[13px] text-muted">{t('onb.install.working.body')}</p>
        </div>
      ) : (
        <div className="space-y-3">
          {phase === 'cancelled' ? (
            <Banner tone="warning" role="alert">
              {t('onb.install.cancelled')}
            </Banner>
          ) : null}
          {phase === 'failed' ? (
            <Banner tone="danger" role="alert">
              {t('service.failed')}
            </Banner>
          ) : null}
          {windows && s.installer_available ? (
            <Button
              size="lg"
              variant="primary"
              icon={<ArrowRight className="size-4" aria-hidden />}
              onClick={() =>
                void run(s.state === 'outdated' ? 'upgrade' : 'install').then(
                  (p) => p === 'done' && onReady(),
                )
              }
            >
              {t('onb.install.cta')}
            </Button>
          ) : s && !windows ? (
            <Banner title={t('service.manual.title')}>
              <code className="font-mono text-xs">{t('service.manual.command')}</code>
            </Banner>
          ) : s ? (
            <Banner tone="warning">{t('service.binary_missing')}</Banner>
          ) : (
            <Spinner label={t('common.loading')} />
          )}
          {windows ? <p className="text-xs text-subtle">{t('onb.install.uac')}</p> : null}
          {!windows && s ? (
            <Button
              icon={<RefreshCw className="size-4" aria-hidden />}
              onClick={() => {
                void app.reconnect();
                status.reload();
              }}
            >
              {t('onb.install.check')}
            </Button>
          ) : null}
        </div>
      )}
    </div>
  );
}

// --- Step 2: hardware -------------------------------------------------------------------------

function Hardware({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const actions = useActions();
  const [busy, setBusy] = useState(false);
  const running = snap.conflicts.filter((c) => c.running);
  const cpu = readSensor(snap, ENTITY.cpuTemp);
  const needsDeep = cpu.value === null && cpu.reason === 'requires_deep_hardware';
  const zones = snap.manifest.entities.filter((e) => e.platform === 'light').length;
  const caps: [LucideIcon, string, boolean][] = [
    [Fan, t('nav.cooling'), hasEntity(snap, ENTITY.pumpDuty) || hasEntity(snap, ENTITY.fanDuty)],
    [Lightbulb, t('nav.lighting'), zones > 0],
    [
      Cpu,
      t('dash.temps.title'),
      snap.manifest.entities.some((e) => e.device_class === 'temperature'),
    ],
  ];
  return (
    <div className="space-y-6">
      <Heading icon={Cpu} title={t('onb.hw.title')}>
        {t('onb.hw.body', { name: snap.manifest.host.name })}
      </Heading>
      <ul className="grid grid-cols-3 gap-2.5">
        {caps.map(([Icon, label, ok]) => (
          <li key={label} className="rounded-xl border border-line bg-surface p-3.5">
            <Icon className="size-[18px] text-subtle" aria-hidden />
            <p className="mt-2 text-sm font-medium text-fg">{label}</p>
            <Badge tone={ok ? 'success' : 'neutral'} className="mt-1.5">
              {ok ? t('onb.hw.found') : t('onb.hw.none')}
            </Badge>
          </li>
        ))}
      </ul>
      {snap.manifest.child_devices.length > 0 ? (
        <ul className="divide-y divide-line rounded-xl border border-line bg-surface px-4 text-sm">
          {snap.manifest.child_devices.map((d) => (
            <li key={d.id} className="flex items-center justify-between gap-3 py-2.5">
              <span className="font-medium text-fg">{d.name}</span>
              <span className="text-xs text-muted">{d.manufacturer ?? ''}</span>
            </li>
          ))}
        </ul>
      ) : null}

      {running.length > 0 ? (
        <div className="space-y-2">
          {running.map((c) => (
            <Banner key={c.id} tone="warning" title={t('alert.conflict.title', { name: c.name })}>
              {t('hw.conflicts.hint')}
            </Banner>
          ))}
          <Button
            icon={<RefreshCw className="size-4" aria-hidden />}
            loading={busy}
            onClick={() =>
              void (async () => {
                setBusy(true);
                await actions.snapshotCall(() => daemon.rescanHardware());
                setBusy(false);
              })()
            }
          >
            {t('onb.hw.rescan')}
          </Button>
        </div>
      ) : (
        <Banner tone="success">{t('hw.conflicts.none')}</Banner>
      )}

      {needsDeep ? (
        <div className="rounded-xl border border-line bg-surface px-4">
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
        </div>
      ) : null}
    </div>
  );
}

// --- Step 3: cooling + safety check -----------------------------------------------------------

function Cooling({ snap }: { snap: DaemonSnapshot }) {
  const { t, temp, pct } = useI18n();
  const policy = snap.cooling_policy;
  const readings = [ENTITY.cpuTemp, ENTITY.gpuTemp, ENTITY.liquidTemp].filter(
    (id) => numberOf(snap, id) !== null,
  ).length;
  const failsafe = policy?.state === 'failsafe';
  const checks: { ok: boolean; label: string; detail: string }[] = [
    {
      ok: readings > 0,
      label: t('onb.cool.check.sensors'),
      detail: t('onb.cool.check.sensors.detail', { count: readings }),
    },
    {
      ok: policy !== null,
      label: t('onb.cool.check.limits'),
      detail: policy
        ? `${t('cooling.source.cpu')} ${temp(policy.thresholds.cpu_critical_c)} · ${t('cooling.source.gpu')} ${temp(policy.thresholds.gpu_critical_c)} · ${t('cooling.source.liquid')} ${temp(policy.thresholds.liquid_critical_c)}`
        : t('cooling.policy.missing.title'),
    },
    {
      ok: policy !== null,
      label: t('onb.cool.check.pump'),
      detail: t('cooling.rule.pump', { min: pct(50) }),
    },
    {
      ok: !failsafe && policy !== null,
      label: t('onb.cool.check.watch'),
      detail: failsafe ? t('cooling.failsafe.title') : t('onb.cool.check.watch.detail'),
    },
  ];
  const all = checks.every((c) => c.ok);
  return (
    <div className="space-y-6">
      <Heading icon={Fan} title={t('onb.cool.title')}>
        {t('onb.cool.body')}
      </Heading>
      <PresetCards snap={snap} />
      <div>
        <div className="mb-2 flex items-center justify-between">
          <h2 className="text-sm font-semibold text-fg">{t('onb.cool.check.title')}</h2>
          <Badge tone={all ? 'success' : 'warning'} dot>
            {all ? t('onb.cool.check.ok') : t('onb.cool.check.warn')}
          </Badge>
        </div>
        <ul className="divide-y divide-line rounded-xl border border-line bg-surface px-4">
          {checks.map((c, i) => (
            <li
              key={c.label}
              className="cm-enter flex items-start gap-3 py-3"
              style={{ animationDelay: `${i * 140}ms` }}
            >
              <span
                className={cn(
                  'mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full',
                  c.ok ? 'bg-success-soft text-success' : 'bg-warning-soft text-warning',
                )}
                aria-hidden
              >
                <Check className="size-3" />
              </span>
              <div>
                <p className="text-sm font-medium text-fg">{c.label}</p>
                <p className="text-[13px] text-muted tabular">{c.detail}</p>
              </div>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

// --- Step 4: Home Assistant -------------------------------------------------------------------

function HomeAssistant({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const pushToast = useDaemon((s) => s.pushToast);
  const paired = snap.paired_clients;
  const copy = async () => {
    const ok = await copyText(REPO_URL);
    pushToast(ok ? 'success' : 'error', ok ? t('common.copied') : t('common.copy_failed'));
  };
  return (
    <div className="space-y-6">
      <Heading icon={Home} title={t('onb.ha.title')}>
        {t('onb.ha.body')}
      </Heading>
      {paired.length > 0 ? (
        <Banner tone="success" title={t('onb.ha.done')} role="status">
          {paired.map((c) => c.client_name).join(', ')}
        </Banner>
      ) : (
        <ol className="space-y-4">
          <li className="flex gap-3">
            <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-surface-2 text-xs font-semibold text-muted">
              1
            </span>
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium text-fg">{t('onb.ha.step1')}</p>
              <div className="mt-2 flex items-center gap-2 rounded-lg bg-surface-2 px-3 py-2">
                <code className="min-w-0 flex-1 truncate font-mono text-xs text-fg">
                  {REPO_URL}
                </code>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Copy className="size-3.5" aria-hidden />}
                  onClick={() => void copy()}
                >
                  {t('common.copy')}
                </Button>
              </div>
            </div>
          </li>
          <li className="flex gap-3">
            <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-surface-2 text-xs font-semibold text-muted">
              2
            </span>
            <p className="text-sm font-medium text-fg">{t('onb.ha.step2')}</p>
          </li>
          <li className="flex gap-3">
            <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-surface-2 text-xs font-semibold text-muted">
              3
            </span>
            <div className="min-w-0 flex-1">
              <p className="mb-3 text-sm font-medium text-fg">{t('onb.ha.step3')}</p>
              <PairingPanel snap={snap} />
            </div>
          </li>
        </ol>
      )}
    </div>
  );
}

// --- Step 5: preferences ----------------------------------------------------------------------

function Prefs({ snap }: { snap: DaemonSnapshot | null }) {
  const { t } = useI18n();
  const actions = useActions();
  const language = usePrefs((s) => s.language);
  const theme = usePrefs((s) => s.theme);
  const { setLanguage, setTheme } = usePrefs.getState();
  const win = useAppSettings();
  return (
    <div className="space-y-6">
      <Heading icon={Sparkles} title={t('onb.prefs.title')}>
        {t('onb.prefs.body')}
      </Heading>
      <div className="divide-y divide-line rounded-xl border border-line bg-surface px-4">
        <Row label={t('settings.language')}>
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
        <Row label={t('settings.theme')}>
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
        {snap ? (
          <Row label={t('settings.unit')}>
            <Segmented<TemperatureUnit>
              size="sm"
              label={t('settings.unit')}
              value={snap.temperature_unit}
              onChange={(u) => void actions.settings({ temperature_unit: u })}
              options={[
                { value: 'celsius', label: '°C' },
                { value: 'fahrenheit', label: '°F' },
              ]}
            />
          </Row>
        ) : null}
        {win.value ? (
          <>
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
          </>
        ) : null}
      </div>
    </div>
  );
}

// --- Shell ------------------------------------------------------------------------------------

export function Onboarding() {
  const { t } = useI18n();
  const snap = useDaemon((s) => s.snapshot);
  const setOnboarded = usePrefs((s) => s.setOnboarded);
  const [index, setIndex] = useState(0);
  const step: Step = STEPS[index] ?? 'welcome';
  const last = index === STEPS.length - 1;
  const optional = step === 'ha' || step === 'hardware';
  const canNext = step === 'welcome' ? snap !== null : true;

  const body =
    step === 'welcome' ? (
      <Welcome snap={snap} onReady={() => undefined} />
    ) : step === 'prefs' ? (
      <Prefs snap={snap} />
    ) : snap === null ? (
      <Banner tone="warning">{t('connection.reconnecting')}</Banner>
    ) : step === 'hardware' ? (
      <Hardware snap={snap} />
    ) : step === 'cooling' ? (
      <Cooling snap={snap} />
    ) : (
      <HomeAssistant snap={snap} />
    );

  return (
    <div className="flex h-full min-h-0">
      <aside className="hidden w-64 shrink-0 flex-col border-r border-line bg-surface px-6 py-8 md:flex">
        <div className="mb-10 flex items-center gap-2.5">
          <span
            className="flex size-7 items-center justify-center rounded-lg bg-accent text-[13px] font-bold text-accent-fg"
            aria-hidden
          >
            C
          </span>
          <span className="text-[15px] font-semibold tracking-tight text-fg">Ctrlmate</span>
        </div>
        <ol className="space-y-1" aria-label={t('onb.steps')}>
          {STEPS.map((s, i) => (
            <li
              key={s}
              aria-current={i === index ? 'step' : undefined}
              className={cn(
                'flex items-center gap-3 rounded-lg px-3 py-2 text-sm',
                i === index ? 'bg-accent-soft font-medium text-accent-text' : 'text-muted',
              )}
            >
              <span
                className={cn(
                  'flex size-5 items-center justify-center rounded-full text-[11px] font-semibold',
                  i < index
                    ? 'bg-success-soft text-success'
                    : i === index
                      ? 'bg-accent text-accent-fg'
                      : 'bg-surface-2 text-subtle',
                )}
              >
                {i < index ? <Check className="size-3" aria-hidden /> : i + 1}
              </span>
              {t(`onb.step.${s}`)}
            </li>
          ))}
        </ol>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex gap-1 px-6 pt-4 md:hidden" aria-hidden>
          {STEPS.map((s, i) => (
            <span
              key={s}
              className={cn('h-1 flex-1 rounded-full', i <= index ? 'bg-accent' : 'bg-line-strong')}
            />
          ))}
        </div>
        <main className="flex min-h-0 flex-1 overflow-y-auto">
          <div key={step} className="m-auto w-full max-w-[560px] px-6 py-10">
            {body}
          </div>
        </main>
        <footer className="flex items-center justify-between border-t border-line bg-surface px-6 py-3.5">
          <div>
            {index > 0 ? (
              <Button
                variant="ghost"
                icon={<ArrowLeft className="size-4" aria-hidden />}
                onClick={() => setIndex(index - 1)}
              >
                {t('common.back')}
              </Button>
            ) : (
              <Button variant="ghost" onClick={() => setOnboarded(true)}>
                {t('onb.later')}
              </Button>
            )}
          </div>
          <div className="flex items-center gap-2">
            {optional ? (
              <Button variant="ghost" onClick={() => setIndex(index + 1)}>
                {t('common.skip')}
              </Button>
            ) : null}
            {last ? (
              <Button
                variant="primary"
                icon={<Check className="size-4" aria-hidden />}
                onClick={() => setOnboarded(true)}
              >
                {t('onb.finish')}
              </Button>
            ) : (
              <Button variant="primary" disabled={!canNext} onClick={() => setIndex(index + 1)}>
                {t('common.continue')}
              </Button>
            )}
          </div>
        </footer>
      </div>
    </div>
  );
}
