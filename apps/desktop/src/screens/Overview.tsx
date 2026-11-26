import { useMemo } from 'react';
import { CheckCircle2 } from 'lucide-react';
import { MetricCard } from '../components/MetricCard';
import { Badge, Banner, Card, KeyValue, PageHeader, type Tone } from '../components/ui/Layout';
import { useI18n } from '../i18n';
import { numberOf, readSensor } from '../lib/snapshot';
import { useDaemon } from '../store/daemon';
import { useNav } from '../store/nav';
import { ENTITY, type DaemonSnapshot } from '../types/protocol';
import { Button } from '../components/ui/Button';

interface Alert {
  id: string;
  tone: Tone;
  title: string;
  body?: string;
  goTo?: 'cooling' | 'system' | 'settings';
}

/** Display name of an entity: "<child device> <entity>" from the manifest, else its id. */
function entityLabel(snap: DaemonSnapshot, id: string): string {
  const e = snap.manifest.entities.find((x) => x.id === id);
  if (!e) return id;
  const child = snap.manifest.child_devices.find((c) => c.id === e.child_device_id);
  return child ? `${child.name} · ${e.name}` : e.name;
}

export function buildAlerts(
  snap: DaemonSnapshot,
  t: ReturnType<typeof useI18n>['t'],
  code: ReturnType<typeof useI18n>['code'],
): Alert[] {
  const alerts: Alert[] = [];
  const policy = snap.cooling_policy;
  if (policy?.state === 'failsafe' || snap.thermal_failsafe_active) {
    alerts.push({
      id: 'failsafe',
      tone: 'danger',
      title: t('alert.failsafe.title'),
      body: policy?.reason ? code('cooling.reason', policy.reason) : t('alert.failsafe.body'),
      goTo: 'cooling',
    });
  }
  const antiCheat = Object.entries(snap.sensor_status).filter(
    ([, s]) => !s.available && s.reason === 'anti_cheat_active',
  );
  if (antiCheat.length > 0) {
    alerts.push({
      id: 'anticheat',
      tone: 'warning',
      title: t('alert.anticheat.title'),
      body: t('alert.anticheat.body'),
    });
  }
  for (const [id, s] of Object.entries(snap.sensor_status)) {
    if (s.available || s.reason === 'anti_cheat_active') continue;
    alerts.push({
      id: `sensor:${id}`,
      tone: s.reason === 'not_present' ? 'neutral' : 'warning',
      title: t('alert.sensor.title', { name: entityLabel(snap, id) }),
      body: code('sensor', s.reason ?? 'error'),
    });
  }
  for (const c of snap.conflicts.filter((x) => x.running)) {
    alerts.push({
      id: `conflict:${c.id}`,
      tone: 'warning',
      title: t('alert.conflict.title', { name: c.name }),
      body: c.impact,
    });
  }
  if (!snap.service.agent_connected && !snap.service.in_process_session) {
    alerts.push({
      id: 'agent',
      tone: 'neutral',
      title: t('alert.agent.title'),
      body: code('result', 'no_agent'),
      goTo: 'settings',
    });
  }
  if (snap.updates.reboot_required) {
    alerts.push({ id: 'reboot', tone: 'neutral', title: t('alert.reboot.title'), goTo: 'system' });
  }
  return alerts;
}

export function Overview({ snapshot: snap }: { snapshot: DaemonSnapshot }) {
  const { t, code, temp, pct, num, locale } = useI18n();
  const history = useDaemon((s) => s.history);
  const connection = useDaemon((s) => s.connection.status);
  const setScreen = useNav((s) => s.setScreen);
  const crit = snap.cooling_policy?.thresholds;

  const cpuT = readSensor(snap, ENTITY.cpuTemp);
  const gpuT = readSensor(snap, ENTITY.gpuTemp);
  const liqT = readSensor(snap, ENTITY.liquidTemp);
  const cpuL = readSensor(snap, ENTITY.cpuLoad);
  const gpuL = readSensor(snap, ENTITY.gpuLoad);
  const fanD = readSensor(snap, ENTITY.fanDuty);
  const pumpD = readSensor(snap, ENTITY.pumpDuty);
  const fanR = numberOf(snap, ENTITY.fanRpm);
  const pumpR = numberOf(snap, ENTITY.pumpRpm);

  const alerts = useMemo(() => buildAlerts(snap, t, code), [snap, t, code]);
  const host = snap.manifest.host;
  const rpm = (v: number | null) =>
    v === null ? null : `${num(v)}\u202f${locale.startsWith('fr') ? 'tr/min' : 'rpm'}`;
  const tempMetric = (
    label: string,
    r: ReturnType<typeof readSensor>,
    hist: keyof typeof history,
    critical?: number,
  ) => (
    <MetricCard
      label={label}
      value={r.value === null ? null : temp(r.value)}
      reading={r}
      history={history[hist]}
      format={(v) => temp(v)}
      alert={critical !== undefined && r.value !== null && r.value >= critical}
    />
  );

  const haConnected = snap.paired_clients.filter((c) => c.connected).length;

  return (
    <>
      <PageHeader title={t('overview.title')} description={host.name} />

      <div className="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(210px,1fr))]">
        {tempMetric(t('metric.cpu_temp'), cpuT, 'cpuTemp', crit?.cpu_critical_c)}
        <MetricCard
          label={t('metric.cpu_load')}
          value={cpuL.value === null ? null : pct(cpuL.value)}
          reading={cpuL}
          history={history.cpuLoad}
          format={(v) => pct(v)}
          domain={[0, 100]}
        />
        {tempMetric(t('metric.gpu_temp'), gpuT, 'gpuTemp', crit?.gpu_critical_c)}
        <MetricCard
          label={t('metric.gpu_load')}
          value={gpuL.value === null ? null : pct(gpuL.value)}
          reading={gpuL}
          history={history.gpuLoad}
          format={(v) => pct(v)}
          domain={[0, 100]}
        />
        {tempMetric(t('metric.liquid_temp'), liqT, 'liquidTemp', crit?.liquid_critical_c)}
        <MetricCard
          label={t('metric.fan')}
          value={fanD.value === null ? null : pct(fanD.value)}
          secondary={rpm(fanR)}
          reading={fanD}
          history={history.fanDuty}
          format={(v) => pct(v)}
          domain={[0, 100]}
        />
        <MetricCard
          label={t('metric.pump')}
          value={pumpD.value === null ? null : pct(pumpD.value)}
          secondary={rpm(pumpR)}
          reading={pumpD}
          history={history.pumpDuty}
          format={(v) => pct(v)}
          domain={[0, 100]}
        />
      </div>

      <div className="mt-4 grid gap-4 lg:grid-cols-2">
        <Card title={t('overview.alerts.title')} id="alerts">
          {alerts.length === 0 ? (
            <p className="flex items-center gap-2 text-sm text-muted">
              <CheckCircle2 className="size-4 text-success" aria-hidden />
              {t('overview.alerts.none')}
            </p>
          ) : (
            <ul className="space-y-2">
              {alerts.map((a) => (
                <li key={a.id}>
                  <Banner
                    tone={a.tone}
                    title={a.title}
                    role={a.tone === 'danger' ? 'alert' : undefined}
                    actions={
                      a.goTo ? (
                        <Button size="sm" onClick={() => setScreen(a.goTo ?? 'overview')}>
                          {t('overview.alerts.open')}
                        </Button>
                      ) : undefined
                    }
                  >
                    {a.body}
                  </Banner>
                </li>
              ))}
            </ul>
          )}
        </Card>

        <Card title={t('overview.status.title')}>
          <dl className="divide-y divide-line">
            <KeyValue label={t('overview.status.connection')}>
              <Badge tone={connection === 'connected' ? 'success' : 'warning'}>
                {t(`connection.status.${connection}`)}
              </Badge>
            </KeyValue>
            <KeyValue label={t('overview.status.service')}>
              {snap.service.running_as_service
                ? t('overview.status.service.installed')
                : t('overview.status.service.foreground')}
              {snap.service.version ? ` · ${snap.service.version}` : ''}
            </KeyValue>
            <KeyValue label={t('overview.status.agent')}>
              <Badge
                tone={
                  snap.service.agent_connected
                    ? 'success'
                    : snap.service.in_process_session
                      ? 'accent'
                      : 'warning'
                }
              >
                {snap.service.agent_connected
                  ? t('agent.connected')
                  : snap.service.in_process_session
                    ? t('agent.in_process')
                    : t('agent.disconnected')}
              </Badge>
            </KeyValue>
            <KeyValue label={t('overview.status.ha')}>
              {t('overview.status.ha.clients', {
                count: snap.paired_clients.length,
                live: haConnected,
              })}
            </KeyValue>
            <KeyValue label={t('overview.status.mqtt')}>
              <Badge
                tone={snap.mqtt.enabled ? (snap.mqtt_connected ? 'success' : 'warning') : 'neutral'}
              >
                {snap.mqtt.enabled
                  ? snap.mqtt_connected
                    ? t('mqtt.connected')
                    : t('mqtt.disconnected')
                  : t('mqtt.disabled')}
              </Badge>
            </KeyValue>
            <KeyValue label={t('overview.status.cooling')}>
              {t(`preset.${snap.cooling_policy?.effective_preset ?? snap.cooling_preset}`)}
            </KeyValue>
          </dl>
        </Card>
      </div>

      <div className="mt-4">
        <Card title={t('overview.host.title')}>
          <dl className="grid gap-x-8 sm:grid-cols-2">
            <KeyValue label={t('overview.host.name')}>{host.name}</KeyValue>
            <KeyValue label={t('overview.host.model')}>
              {[host.manufacturer, host.model].filter(Boolean).join(' ') || t('common.unknown')}
            </KeyValue>
            <KeyValue label={t('overview.host.os')}>
              {[host.os_name, host.os_version].filter(Boolean).join(' ')}
            </KeyValue>
            <KeyValue label={t('overview.host.security')}>
              {t(`security.${host.security_mode}`)}
            </KeyValue>
          </dl>
          {snap.manifest.child_devices.length > 0 ? (
            <ul className="mt-3 flex flex-wrap gap-2" aria-label={t('overview.host.devices')}>
              {snap.manifest.child_devices.map((c) => (
                <li
                  key={c.id}
                  className="rounded-full border border-line bg-surface-2 px-3 py-1 text-xs text-muted"
                >
                  {c.name}
                </li>
              ))}
            </ul>
          ) : null}
        </Card>
      </div>
    </>
  );
}
