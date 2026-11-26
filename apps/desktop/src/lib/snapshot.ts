import {
  ENTITY,
  type CoolingPolicy,
  type CoolingPreset,
  type DaemonSnapshot,
  type EntityDescriptor,
  type EntityState,
  type LightState,
  type RgbDeviceStatus,
  type SensorStatus,
  type UpdatesInfo,
} from '../types/protocol';

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v);

/** Daemon older than the app (or a partial payload) may omit v0.6 fields: fill safe defaults. */
const EMPTY_UPDATES: UpdatesInfo = {
  checked: false,
  last_checked: null,
  pending_count: null,
  pending: [],
  titles: [],
  kb_ids: [],
  reboot_required: false,
  in_progress: false,
  error: null,
  installed_version: '',
};

/**
 * Validates the minimum shape of a snapshot coming from the daemon and fills defaults for the
 * optional v0.6 additions. Returns `null` when the payload cannot be a snapshot at all.
 */
export function normalizeSnapshot(raw: unknown): DaemonSnapshot | null {
  if (!isObject(raw)) return null;
  const manifest = raw.manifest;
  if (!isObject(manifest) || !isObject(manifest.host)) return null;
  const arr = <T>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
  const s = raw as unknown as Partial<DaemonSnapshot>;
  const policy = isObject(raw.cooling_policy)
    ? (raw.cooling_policy as unknown as CoolingPolicy)
    : null;
  return {
    ...(s as DaemonSnapshot),
    manifest: {
      ...(manifest as unknown as DaemonSnapshot['manifest']),
      child_devices: arr(manifest.child_devices),
      entities: arr(manifest.entities),
    },
    states: arr(raw.states),
    conflicts: arr(raw.conflicts),
    hid_devices: arr(raw.hid_devices),
    paired_clients: arr(raw.paired_clients),
    scripts: arr(raw.scripts),
    rgb_backends: arr(raw.rgb_backends),
    rgb_devices: arr(raw.rgb_devices),
    mqtt_connected: s.mqtt_connected ?? false,
    mqtt_password_set: s.mqtt_password_set ?? false,
    deep_bridge_active: s.deep_bridge_active ?? false,
    thermal_failsafe_active: s.thermal_failsafe_active ?? false,
    cooling_preset: s.cooling_preset ?? 'auto',
    cooling_policy: policy ? { ...policy, presets: arr(policy.presets) } : null,
    sensor_status: isObject(raw.sensor_status)
      ? (raw.sensor_status as Record<string, SensorStatus>)
      : {},
    session: isObject(raw.session) ? (raw.session as unknown as DaemonSnapshot['session']) : null,
    updates: isObject(raw.updates)
      ? { ...EMPTY_UPDATES, ...(raw.updates as Partial<UpdatesInfo>) }
      : EMPTY_UPDATES,
    service: isObject(raw.service)
      ? { ...(raw.service as unknown as DaemonSnapshot['service']) }
      : {
          running_as_service: false,
          version: '',
          protocol_version: 0,
          session_type: 'service',
          agent_connected: false,
          in_process_session: false,
          data_dir_kind: 'user',
        },
  };
}

export function stateOf(snap: DaemonSnapshot, id: string): EntityState | undefined {
  return snap.states.find((s) => s.id === id);
}

/** Numeric value of a sensor entity, `null` when missing / unavailable (never a default). */
export function numberOf(snap: DaemonSnapshot, id: string): number | null {
  const v = stateOf(snap, id)?.value;
  if (!v) return null;
  if ((v.kind === 'float' || v.kind === 'int') && Number.isFinite(v.value)) return v.value;
  return null;
}

export function boolOf(snap: DaemonSnapshot, id: string): boolean | null {
  const v = stateOf(snap, id)?.value;
  return v?.kind === 'bool' ? v.value : null;
}

export interface SensorReading {
  value: number | null;
  available: boolean;
  /** Raw reason code (translate with `sensor.*`), when the daemon gave one. */
  reason: string | null;
}

export function readSensor(snap: DaemonSnapshot, id: string): SensorReading {
  const value = numberOf(snap, id);
  const status = snap.sensor_status[id];
  if (value !== null) return { value, available: true, reason: null };
  return {
    value: null,
    available: false,
    reason: status && !status.available ? (status.reason ?? 'error') : null,
  };
}

export interface LightZone {
  id: string;
  name: string;
  descriptor: EntityDescriptor;
  state: LightState | null;
  isMaster: boolean;
  /** `rgb_devices` rows that feed this zone (transport + reason). */
  devices: RgbDeviceStatus[];
  /** A zone is controllable when its state exists and at least one backing device is. */
  controllable: boolean;
  /** Reason code when the zone cannot be controlled (translate with `rgb.reason.*`). */
  reason: string | null;
}

/** Zones come from the manifest only; capabilities decide the controls. No invented zones. */
export function lightZones(snap: DaemonSnapshot): LightZone[] {
  const zones: LightZone[] = [];
  for (const d of snap.manifest.entities) {
    if (d.platform !== 'light' || d.exposed === false) continue;
    const st = stateOf(snap, d.id)?.value;
    const state = st?.kind === 'light' ? st.value : null;
    const devices = snap.rgb_devices.filter((r) => r.zone_id === d.id);
    const isMaster = d.light_caps?.is_master_zone ?? d.id === ENTITY.lightGlobal;
    const anyControllable = devices.some((r) => r.controllable);
    const controllable = state !== null && (isMaster || devices.length === 0 || anyControllable);
    const blocked = devices.find((r) => !r.controllable);
    zones.push({
      id: d.id,
      name: d.name,
      descriptor: d,
      state,
      isMaster,
      devices,
      controllable,
      reason: controllable ? null : (blocked?.reason ?? (state === null ? 'unavailable' : null)),
    });
  }
  zones.sort((a, b) => Number(b.isMaster) - Number(a.isMaster));
  return zones;
}

/** RGB devices that have no zone in the manifest (informational, e.g. Samsung NVMe LED). */
export function orphanRgbDevices(snap: DaemonSnapshot): RgbDeviceStatus[] {
  const zoneIds = new Set(
    snap.manifest.entities.filter((e) => e.platform === 'light').map((e) => e.id),
  );
  return snap.rgb_devices.filter((r) => !zoneIds.has(r.zone_id));
}

export function hasEntity(snap: DaemonSnapshot, id: string): boolean {
  return snap.manifest.entities.some((e) => e.id === id);
}

export function presetCurve(policy: CoolingPolicy, preset: CoolingPreset) {
  return policy.presets.find((p) => p.preset === preset);
}

export function hostName(snap: DaemonSnapshot): string {
  return snap.manifest.host.name;
}
