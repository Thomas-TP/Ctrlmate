import type { Params, TKey } from '../i18n/core';
import type { SettingsTab, SheetId } from '../store/ui';
import { ENTITY, type DaemonSnapshot, type EntityDescriptor } from '../types/protocol';
import { numberOf, readSensor, type SensorReading } from './snapshot';

export interface Alert {
  id: string;
  tone: 'danger' | 'warning' | 'neutral';
  key: TKey;
  params?: Params;
  /** Longer explanation shown under the title. */
  bodyKey?: TKey;
  /** Where "Fix it" leads. */
  sheet: SheetId;
  tab?: SettingsTab;
}

/** Everything that deserves the user's attention, most urgent first. Empty = all good. */
export function deriveAlerts(snap: DaemonSnapshot): Alert[] {
  const out: Alert[] = [];
  const failsafe = snap.thermal_failsafe_active || snap.cooling_policy?.state === 'failsafe';
  if (failsafe)
    out.push({
      id: 'failsafe',
      tone: 'danger',
      key: 'alert.failsafe.title',
      bodyKey: 'alert.failsafe.body',
      sheet: 'cooling',
    });
  if (
    Object.values(snap.sensor_status).some((s) => !s.available && s.reason === 'anti_cheat_active')
  ) {
    out.push({
      id: 'anticheat',
      tone: 'warning',
      key: 'alert.anticheat.title',
      bodyKey: 'alert.anticheat.body',
      sheet: 'settings',
      tab: 'hardware',
    });
  }
  for (const c of snap.conflicts) {
    if (c.running) {
      out.push({
        id: `conflict:${c.id}`,
        tone: 'warning',
        key: 'alert.conflict.title',
        params: { name: c.name },
        sheet: 'settings',
        tab: 'hardware',
      });
    }
  }
  const { service } = snap;
  if (!service.agent_connected && !service.in_process_session) {
    out.push({ id: 'agent', tone: 'warning', key: 'alert.agent.title', sheet: 'system' });
  }
  if (snap.updates.reboot_required) {
    out.push({ id: 'reboot', tone: 'neutral', key: 'alert.reboot.title', sheet: 'updates' });
  }
  return out;
}

export interface TempRow {
  id: string;
  name: string;
  reading: SensorReading;
}

const TEMP_PRIORITY: string[] = [ENTITY.cpuTemp, ENTITY.gpuTemp, ENTITY.liquidTemp];

function isTemperature(d: EntityDescriptor): boolean {
  return d.platform === 'sensor' && d.device_class === 'temperature' && d.exposed !== false;
}

/** Temperature sensors known to the manifest: CPU, GPU and liquid first, the rest after. */
export function temperatureRows(snap: DaemonSnapshot): TempRow[] {
  const all = snap.manifest.entities.filter(isTemperature);
  const rank = (id: string) => {
    const i = TEMP_PRIORITY.indexOf(id);
    return i === -1 ? TEMP_PRIORITY.length : i;
  };
  const children = new Map(snap.manifest.child_devices.map((c) => [c.id, c.name]));
  const perChild = new Map<string | null, number>();
  for (const d of all) perChild.set(d.child_device_id, (perChild.get(d.child_device_id) ?? 0) + 1);
  // "Temperature" alone says nothing: name the sensor after its device, and add the entity name
  // only when one device has several temperature sensors.
  const label = (d: EntityDescriptor): string => {
    const device = d.child_device_id ? children.get(d.child_device_id) : undefined;
    if (!device) return d.name;
    return (perChild.get(d.child_device_id) ?? 0) > 1 && d.name !== 'Temperature'
      ? `${device} · ${d.name}`
      : device;
  };
  return [...all]
    .sort((a, b) => rank(a.id) - rank(b.id))
    .map((d) => ({ id: d.id, name: label(d), reading: readSensor(snap, d.id) }));
}

/** The hottest of CPU, GPU and liquid right now, or `null` when none is readable. */
export function hottest(snap: DaemonSnapshot): number | null {
  const v = TEMP_PRIORITY.map((id) => numberOf(snap, id)).filter((n): n is number => n !== null);
  return v.length ? Math.max(...v) : null;
}

/** Maps a temperature to a 0-100 bar fill (20 °C empty, 100 °C full). */
export const tempFill = (celsius: number): number =>
  Math.min(100, Math.max(0, ((celsius - 20) / 80) * 100));
