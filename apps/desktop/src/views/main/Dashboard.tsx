import type { ReactNode } from 'react';
import {
  ChevronRight,
  Download,
  Fan,
  Home,
  Lightbulb,
  MonitorCog,
  Thermometer,
  Volume2,
} from 'lucide-react';
import { cmd } from '../../api/daemon';
import { useAppUpdate } from '../../hooks/hooks';
import { useI18n } from '../../i18n';
import { deriveAlerts, temperatureRows, tempFill, type Alert } from '../../lib/derive';
import { hostName, numberOf, readSensor } from '../../lib/snapshot';
import { useActions } from '../../store/actions';
import { useDaemon } from '../../store/daemon';
import { useUi, type SheetId } from '../../store/ui';
import { ENTITY, type DaemonSnapshot } from '../../types/protocol';
import { Sparkline } from '../../ui/charts';
import { Badge, Banner, Button, Slider, cn } from '../../ui/kit';
import { MasterLight, PresetPicker, QuickActions } from '../shared/controls';

const INTERACTIVE = 'button,input,select,a,label,[role=radio],[role=switch],[role=radiogroup]';

/** A dashboard card: the whole surface opens the detail panel, inner controls stay live. */
function Tile({
  title,
  icon,
  sheet,
  className,
  aside,
  children,
}: {
  title: string;
  icon: ReactNode;
  sheet: SheetId;
  className?: string;
  aside?: ReactNode;
  children: ReactNode;
}) {
  const openSheet = useUi((s) => s.openSheet);
  return (
    <section
      onClick={(e) => {
        if (!(e.target as HTMLElement).closest(INTERACTIVE)) openSheet(sheet);
      }}
      className={cn(
        'group flex cursor-pointer flex-col rounded-card border border-line bg-surface p-5 transition-all duration-150 hover:border-line-strong hover:shadow-[0_6px_24px_rgb(0_0_0/0.06)]',
        className,
      )}
    >
      <header className="mb-4 flex items-center justify-between gap-2">
        <button
          type="button"
          onClick={() => openSheet(sheet)}
          className="flex items-center gap-2 rounded-md text-[13px] font-medium text-muted transition-colors group-hover:text-fg"
        >
          <span className="text-subtle" aria-hidden>
            {icon}
          </span>
          {title}
          <ChevronRight
            className="size-3.5 -translate-x-1 opacity-0 transition-all group-hover:translate-x-0 group-hover:opacity-100"
            aria-hidden
          />
        </button>
        {aside}
      </header>
      {children}
    </section>
  );
}

// --- Hero -------------------------------------------------------------------------------------

function Hero({ snap, alerts }: { snap: DaemonSnapshot; alerts: Alert[] }) {
  const { t, temp } = useI18n();
  const cpu = numberOf(snap, ENTITY.cpuTemp);
  const gpu = numberOf(snap, ENTITY.gpuTemp);
  const liquid = numberOf(snap, ENTITY.liquidTemp);
  const failsafe = alerts.some((a) => a.id === 'failsafe');
  const parts = [
    cpu !== null && `${t('dash.short.cpu')} ${temp(cpu)}`,
    gpu !== null && `${t('dash.short.gpu')} ${temp(gpu)}`,
    liquid !== null && `${t('dash.short.liquid')} ${temp(liquid)}`,
  ].filter(Boolean);
  const title = failsafe
    ? t('dash.hero.failsafe')
    : alerts.length > 0
      ? t('dash.hero.attention', { count: alerts.length })
      : t('dash.hero.ok');
  return (
    <div className="cm-enter">
      <p className="text-[13px] font-medium text-subtle">{hostName(snap)}</p>
      <h1 className="mt-1 text-[30px] leading-tight font-semibold tracking-tight text-fg">
        {title}
      </h1>
      <p className="mt-1 text-[15px] text-muted tabular">
        {parts.length > 0 ? parts.join('  ·  ') : t('dash.hero.no_sensor')}
      </p>
    </div>
  );
}

function Alerts({ alerts }: { alerts: Alert[] }) {
  const { t } = useI18n();
  const open = useUi((s) => s.openSheet);
  if (alerts.length === 0) return null;
  return (
    <div className="space-y-2">
      {alerts.map((a) => (
        <Banner
          key={a.id}
          tone={a.tone}
          role={a.tone === 'danger' ? 'alert' : undefined}
          title={t(a.key, a.params)}
          action={
            <Button size="sm" variant="ghost" onClick={() => open(a.sheet, a.tab)}>
              {t('overview.alerts.open')}
            </Button>
          }
        >
          {a.bodyKey ? t(a.bodyKey) : null}
        </Banner>
      ))}
    </div>
  );
}

// --- Tiles ------------------------------------------------------------------------------------

function CoolingTile({ snap }: { snap: DaemonSnapshot }) {
  const { t, temp, pct } = useI18n();
  const history = useDaemon((s) => s.history);
  const liquid = readSensor(snap, ENTITY.liquidTemp);
  const cpu = readSensor(snap, ENTITY.cpuTemp);
  const main = liquid.value !== null ? liquid : cpu;
  const series = liquid.value !== null ? history.liquidTemp : history.cpuTemp;
  const failsafe = snap.cooling_policy?.state === 'failsafe' || snap.thermal_failsafe_active;
  const pump = numberOf(snap, ENTITY.pumpDuty);
  const fan = numberOf(snap, ENTITY.fanDuty);
  return (
    <Tile
      title={t('nav.cooling')}
      icon={<Fan className="size-4" />}
      sheet="cooling"
      className="lg:col-span-7"
      aside={
        <Badge tone={failsafe ? 'danger' : 'success'} dot>
          {failsafe ? t('cooling.state.failsafe') : t('cooling.state.normal')}
        </Badge>
      }
    >
      <div className="flex items-end justify-between gap-4">
        <div>
          <p className="text-[44px] leading-none font-semibold tracking-tight text-fg tabular">
            {temp(main.value)}
          </p>
          <p className="mt-1.5 text-[13px] text-muted">
            {liquid.value !== null ? t('metric.liquid_temp') : t('metric.cpu_temp')}
          </p>
        </div>
        <dl className="text-right text-[13px] text-muted tabular">
          <div>
            {t('metric.pump')} <dd className="ml-1 inline font-medium text-fg">{pct(pump)}</dd>
          </div>
          <div>
            {t('metric.fan')} <dd className="ml-1 inline font-medium text-fg">{pct(fan)}</dd>
          </div>
        </dl>
      </div>
      <div className="my-3 -mx-1">
        <Sparkline points={series} height={64} />
      </div>
      <div className="mt-auto">
        <PresetPicker snap={snap} size="sm" full />
      </div>
    </Tile>
  );
}

function TempsTile({ snap }: { snap: DaemonSnapshot }) {
  const { t, temp, code } = useI18n();
  const rows = temperatureRows(snap).slice(0, 5);
  return (
    <Tile
      title={t('dash.temps.title')}
      icon={<Thermometer className="size-4" />}
      sheet="temps"
      className="lg:col-span-5"
    >
      {rows.length === 0 ? (
        <p className="text-[13px] text-muted">{t('dash.hero.no_sensor')}</p>
      ) : (
        <ul className="space-y-3.5">
          {rows.map((r) => (
            <li key={r.id}>
              <div className="mb-1.5 flex items-baseline justify-between gap-3 text-sm">
                <span className="truncate text-fg">{r.name}</span>
                {r.reading.value !== null ? (
                  <span className="font-medium text-fg tabular">{temp(r.reading.value)}</span>
                ) : (
                  <span
                    className="text-subtle"
                    title={r.reading.reason ? code('sensor', r.reading.reason) : undefined}
                  >
                    {t('common.unavailable')}
                  </span>
                )}
              </div>
              <div className="h-1.5 overflow-hidden rounded-full bg-surface-2">
                <div
                  className="h-full rounded-full bg-accent transition-[width] duration-500"
                  style={{ width: `${r.reading.value === null ? 0 : tempFill(r.reading.value)}%` }}
                />
              </div>
            </li>
          ))}
        </ul>
      )}
    </Tile>
  );
}

function LightingTile({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const hasZones = snap.manifest.entities.some((e) => e.platform === 'light');
  return (
    <Tile
      title={t('nav.lighting')}
      icon={<Lightbulb className="size-4" />}
      sheet="lighting"
      className="lg:col-span-4"
    >
      {hasZones ? (
        <MasterLight snap={snap} compact />
      ) : (
        <p className="text-[13px] text-muted">{t('lighting.empty.title')}</p>
      )}
    </Tile>
  );
}

function PcTile({ snap }: { snap: DaemonSnapshot }) {
  const { t, num } = useI18n();
  const actions = useActions();
  const audio = snap.session?.audio ?? null;
  const app = snap.session?.foreground_app;
  return (
    <Tile
      title={t('dash.pc.title')}
      icon={<MonitorCog className="size-4" />}
      sheet="system"
      className="lg:col-span-4"
      aside={snap.session?.gaming_mode ? <Badge tone="accent">{t('dash.pc.gaming')}</Badge> : null}
    >
      <div className="space-y-4">
        <QuickActions snap={snap} />
        {audio ? (
          <div className="flex items-center gap-3">
            <Volume2 className="size-4 shrink-0 text-subtle" aria-hidden />
            <div className="flex-1">
              <Slider
                hideLabel
                label={t('system.audio.volume')}
                value={Math.round(audio.volume)}
                min={0}
                max={100}
                format={(v) => `${num(v)}\u202f%`}
                onCommit={(v) => void actions.run(cmd.volume(v))}
              />
            </div>
          </div>
        ) : null}
        {app?.process_name ? (
          <p className="truncate text-[13px] text-muted">
            {t('system.session.app')} <span className="text-fg">{app.process_name}</span>
          </p>
        ) : null}
      </div>
    </Tile>
  );
}

function HaTile({ snap }: { snap: DaemonSnapshot }) {
  const { t } = useI18n();
  const open = useUi((s) => s.openSheet);
  const live = snap.paired_clients.filter((c) => c.connected).length;
  const paired = snap.paired_clients.length;
  return (
    <Tile
      title={t('nav.homeAssistant')}
      icon={<Home className="size-4" />}
      sheet="ha"
      className="lg:col-span-4"
    >
      {paired === 0 ? (
        <div className="space-y-3">
          <p className="text-[13px] text-muted">{t('dash.ha.idle')}</p>
          <Button size="sm" variant="primary" onClick={() => open('ha')}>
            {t('dash.ha.cta')}
          </Button>
        </div>
      ) : (
        <div className="space-y-1">
          <Badge tone={live > 0 ? 'success' : 'warning'} dot>
            {live > 0 ? t('dash.ha.connected') : t('ha.clients.offline')}
          </Badge>
          <p className="pt-2 text-[13px] text-muted">
            {t('overview.status.ha.clients', { count: paired, live })}
          </p>
        </div>
      )}
    </Tile>
  );
}

function UpdatesStrip({ snap }: { snap: DaemonSnapshot }) {
  const { t, num } = useI18n();
  const open = useUi((s) => s.openSheet);
  const appUpdate = useAppUpdate().data;
  const u = snap.updates;
  const os = !u.checked
    ? t('system.updates.not_checked')
    : (u.pending_count ?? 0) > 0
      ? t('system.updates.pending', {
          count: u.pending_count ?? 0,
          formatted: num(u.pending_count ?? 0),
        })
      : t('system.updates.up_to_date');
  const appLine =
    appUpdate?.state === 'available'
      ? t('settings.update.available', { version: appUpdate.latest_version ?? '' })
      : appUpdate?.state === 'up_to_date'
        ? t('settings.update.up_to_date')
        : null;
  return (
    <section className="flex flex-wrap items-center justify-between gap-3 rounded-card border border-line bg-surface px-5 py-3.5 lg:col-span-12">
      <div className="flex items-center gap-2 text-[13px] text-muted">
        <Download className="size-4 text-subtle" aria-hidden />
        <span>
          <span className="font-medium text-fg">Windows</span> · {os}
        </span>
        {appLine ? (
          <span>
            <span className="mx-1 text-line-strong">|</span>
            <span className="font-medium text-fg">Ctrlmate</span> · {appLine}
          </span>
        ) : null}
      </div>
      <Button size="sm" variant="ghost" onClick={() => open('updates')}>
        {t('dash.updates.manage')}
      </Button>
    </section>
  );
}

export function Dashboard({ snap }: { snap: DaemonSnapshot }) {
  const alerts = deriveAlerts(snap);
  return (
    <div className="mx-auto w-full max-w-[1120px] space-y-6 px-6 pt-8 pb-12">
      <Hero snap={snap} alerts={alerts} />
      <Alerts alerts={alerts} />
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-12">
        <CoolingTile snap={snap} />
        <TempsTile snap={snap} />
        <LightingTile snap={snap} />
        <PcTile snap={snap} />
        <HaTile snap={snap} />
        <UpdatesStrip snap={snap} />
      </div>
    </div>
  );
}
