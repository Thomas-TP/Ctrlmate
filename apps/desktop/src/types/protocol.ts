// TypeScript mirror of `crates/ctrlmate-protocol` (wire JSON sent by ctrlmated over /ipc/*).
// Fields that Rust skips when empty (`skip_serializing_if`) are optional here; `Option<T>` that
// serialises as `null` is `T | null`. Always read optional values defensively.

export const MIN_COLOR_TEMP_KELVIN = 2000;
export const MAX_COLOR_TEMP_KELVIN = 6500;
export const MIN_PUMP_PERCENT = 50;
export const CPU_CRITICAL_MIN_C = 85;
export const CPU_CRITICAL_MAX_C = 98;

export const COOLING_PRESETS = ['auto', 'silent', 'balanced', 'performance', 'gaming'] as const;
export type CoolingPreset = (typeof COOLING_PRESETS)[number];

export const POLLING_PROFILES = ['intelligent', 'eco', 'balanced', 'realtime'] as const;
export type PollingProfile = (typeof POLLING_PROFILES)[number];

export const TEMPERATURE_UNITS = ['celsius', 'fahrenheit'] as const;
export type TemperatureUnit = (typeof TEMPERATURE_UNITS)[number];

export type SecurityMode = 'zero_kernel_safe' | 'deep_hardware_pawn_io';

export const RESULT_CODES = [
  'ok',
  'permission_denied',
  'no_agent',
  'agent_timeout',
  'not_supported',
  'invalid_argument',
  'hardware_error',
  'busy',
  'unavailable',
  'internal',
] as const;
export type ResultCode = (typeof RESULT_CODES)[number];

export interface CommandResult {
  ok: boolean;
  /** One of RESULT_CODES, but a newer daemon may send others. */
  code: string;
  /** Technical detail only (e.g. script output); never a UI sentence. */
  message?: string | null;
}

export const SENSOR_REASONS = [
  'not_present',
  'requires_deep_hardware',
  'anti_cheat_active',
  'error',
  'permission',
] as const;
export type SensorReason = (typeof SENSOR_REASONS)[number];

export interface SensorStatus {
  available: boolean;
  // A newer daemon may send reasons this app does not know yet (see `SENSOR_REASONS`).
  reason?: string | null;
}

export interface RemotePermissions {
  telemetry: boolean;
  lighting: boolean;
  cooling: boolean;
  audio_and_media: boolean;
  power_control: boolean;
  os_updates: boolean;
  notifications: boolean;
  custom_scripts: boolean;
}

export type PermissionKey = keyof RemotePermissions;
export const PERMISSION_KEYS: readonly PermissionKey[] = [
  'telemetry',
  'lighting',
  'cooling',
  'audio_and_media',
  'power_control',
  'os_updates',
  'notifications',
  'custom_scripts',
];

export type DeviceCategory =
  'system' | 'cpu' | 'gpu' | 'motherboard' | 'memory' | 'storage' | 'cooler' | 'lighting' | 'audio';

export interface HostDeviceInfo {
  device_id: string;
  name: string;
  manufacturer: string;
  model: string;
  os_name: string;
  os_version: string;
  sw_version: string;
  mac_address: string | null;
  security_mode: SecurityMode;
}

export interface ChildDeviceInfo {
  id: string;
  name: string;
  category: DeviceCategory;
  manufacturer: string | null;
  model: string | null;
  hw_version: string | null;
}

export type EntityPlatform =
  'sensor' | 'binary_sensor' | 'light' | 'select' | 'button' | 'number' | 'switch' | 'update';

export interface NumberRange {
  min: number;
  max: number;
  step: number;
}

export interface LightCapabilities {
  supports_rgb: boolean;
  supports_color_temp: boolean;
  min_color_temp_kelvin: number;
  max_color_temp_kelvin: number;
  supports_transition: boolean;
  is_master_zone: boolean;
  led_count: number;
}

export interface EntityDescriptor {
  id: string;
  name: string;
  platform: EntityPlatform;
  child_device_id: string | null;
  device_class: string | null;
  state_class: string | null;
  unit_of_measurement: string | null;
  icon: string | null;
  entity_category: 'config' | 'diagnostic' | null;
  suggested_display_precision: number | null;
  options?: string[];
  number_range?: NumberRange | null;
  light_caps?: LightCapabilities | null;
  exposed?: boolean;
}

export interface DeviceManifest {
  protocol_version: number;
  host: HostDeviceInfo;
  child_devices: ChildDeviceInfo[];
  entities: EntityDescriptor[];
}

export interface RgbColor {
  r: number;
  g: number;
  b: number;
}

export interface LightState {
  is_on: boolean;
  /** 0-255 */
  brightness: number;
  color_mode: 'rgb' | 'color_temp';
  rgb: RgbColor;
  color_temp_kelvin: number;
}

export interface PendingUpdate {
  title: string;
  kb?: string | null;
}

export interface UpdatesInfo {
  checked: boolean;
  last_checked: string | null;
  pending_count: number | null;
  pending: PendingUpdate[];
  titles: string[];
  kb_ids: string[];
  reboot_required: boolean;
  in_progress: boolean;
  error?: string | null;
  installed_version: string;
}

export type EntityValue =
  | { kind: 'float'; value: number }
  | { kind: 'int'; value: number }
  | { kind: 'bool'; value: boolean }
  | { kind: 'text'; value: string }
  | { kind: 'light'; value: LightState }
  | { kind: 'update'; value: UpdatesInfo }
  | { kind: 'unavailable' };

export interface EntityState {
  id: string;
  value: EntityValue;
  attributes?: Record<string, unknown>;
  updated_at: string;
}

export interface SoftwareConflict {
  id: string;
  name: string;
  category: string;
  running: boolean;
  impact: string;
}

export interface DetectedHidDevice {
  vendor_id: number;
  product_id: number;
  manufacturer: string;
  product: string;
  driver_hint: string;
  path: string;
}

export interface RgbDeviceStatus {
  id: string;
  name: string;
  zone_id: string;
  transport: string;
  controllable: boolean;
  /** `ok`, `requires_deep_hardware`, `via_openrgb`, `native_*`, ... (see i18n `rgb.reason.*`). */
  reason: string;
}

export interface MqttConfig {
  enabled: boolean;
  host: string;
  port: number;
  username: string | null;
  /** Never returned by the daemon; send `""` to clear, omit/null to keep. */
  password: string | null;
  use_tls: boolean;
  discovery_prefix: string;
}

export interface AllowlistedScript {
  id: string;
  name: string;
  executable: string;
  args: string[];
  working_dir: string | null;
  enabled: boolean;
}

export interface PairingSessionInfo {
  active: boolean;
  pin: string | null;
  expires_at: string | null;
  cert_fingerprint: string;
  listen_port: number;
}

export interface PairedClientInfo {
  client_id: string;
  client_name: string;
  paired_at: string;
  connected: boolean;
  connections: number;
}

export interface FanCurvePoint {
  temp_c: number;
  duty: number;
}

export interface PresetCurve {
  preset: CoolingPreset;
  fan_curve: FanCurvePoint[];
  pump_floor: number;
}

export interface ThermalThresholds {
  cpu_critical_c: number;
  gpu_critical_c: number;
  liquid_critical_c: number;
  hysteresis_c: number;
  pump_floor_c: number;
}

export type CoolingState = 'normal' | 'failsafe';
export const COOLING_REASONS = ['temperature', 'sensor_lost', 'write_failed'] as const;
export type CoolingReason = (typeof COOLING_REASONS)[number];

export interface CoolingPolicy {
  presets: PresetCurve[];
  thresholds: ThermalThresholds;
  requested_preset: CoolingPreset;
  effective_preset: CoolingPreset;
  state: CoolingState;
  reason?: CoolingReason | null;
}

export interface ForegroundApp {
  process_name: string;
  window_title?: string | null;
}

export interface AudioOutput {
  id: string;
  name: string;
}

export interface AudioState {
  volume: number;
  muted: boolean;
  output_id?: string | null;
  outputs: AudioOutput[];
}

export interface SessionState {
  foreground_app: ForegroundApp | null;
  gaming_mode: boolean;
  locked: boolean;
  idle_seconds: number;
  mic_in_use: boolean | null;
  webcam_in_use: boolean | null;
  audio: AudioState | null;
}

export type SessionType = 'service' | 'interactive';
export type DataDirKind = 'system' | 'user' | 'custom';

export interface ServiceInfo {
  running_as_service: boolean;
  version: string;
  protocol_version: number;
  session_type: SessionType;
  agent_connected: boolean;
  in_process_session: boolean;
  data_dir_kind: DataDirKind;
  agent_version?: string | null;
  agent_session_id?: number | null;
}

export interface DaemonSnapshot {
  manifest: DeviceManifest;
  states: EntityState[];
  conflicts: SoftwareConflict[];
  hid_devices: DetectedHidDevice[];
  pairing: PairingSessionInfo;
  paired_clients: PairedClientInfo[];
  permissions: RemotePermissions;
  polling_profile: PollingProfile;
  temperature_unit: TemperatureUnit;
  scripts: AllowlistedScript[];
  thermal_failsafe_active: boolean;
  rgb_backends: string[];
  rgb_devices: RgbDeviceStatus[];
  deep_bridge_active: boolean;
  mqtt: MqttConfig;
  mqtt_connected: boolean;
  mqtt_password_set: boolean;
  cooling_preset: CoolingPreset;
  /** Always sent by v0.6 daemons; `null` only for a malformed or older payload. */
  cooling_policy: CoolingPolicy | null;
  sensor_status: Record<string, SensorStatus>;
  session: SessionState | null;
  service: ServiceInfo;
  updates: UpdatesInfo;
}

export type ControlCommand =
  | {
      action: 'set_light';
      entity_id: string;
      is_on?: boolean | null;
      brightness?: number | null;
      rgb?: RgbColor | null;
      color_temp_kelvin?: number | null;
      transition_ms?: number | null;
    }
  | { action: 'set_cooling_preset'; entity_id: string; preset: CoolingPreset }
  | { action: 'set_select'; entity_id: string; option: string }
  | { action: 'set_number'; entity_id: string; value: number }
  | { action: 'set_switch'; entity_id: string; state: boolean }
  | { action: 'press_button'; entity_id: string }
  | { action: 'check_os_updates' }
  | { action: 'install_os_updates' }
  | { action: 'send_notification'; title: string; message: string };

export interface SettingsUpdate {
  permissions?: RemotePermissions;
  polling_profile?: PollingProfile;
  temperature_unit?: TemperatureUnit;
  security_mode?: SecurityMode;
  scripts?: AllowlistedScript[];
  mqtt?: MqttConfig;
}

export type IpcEvent = { type: 'auth_ok' } | ({ type: 'snapshot' } & DaemonSnapshot);

/** Well-known entity ids (ARCHITECTURE section 5 + daemon command table). */
export const ENTITY = {
  cpuTemp: 'cpu_0_temperature',
  cpuLoad: 'cpu_0_load',
  cpuPower: 'cpu_0_power',
  gpuTemp: 'gpu_0_temperature',
  gpuLoad: 'gpu_0_load',
  liquidTemp: 'cooler_0_liquid_temperature',
  fanRpm: 'cooler_0_fan_rpm',
  fanDuty: 'cooler_0_fan_duty',
  pumpRpm: 'cooler_0_pump_rpm',
  pumpDuty: 'cooler_0_pump_duty',
  memoryUsage: 'memory_0_usage',
  lightGlobal: 'light_global',
  masterVolume: 'master_volume',
  audioMute: 'audio_mute',
  audioOutput: 'audio_output',
  coolingPreset: 'cooling_preset',
  agentConnected: 'agent_connected',
  osUpdates: 'os_updates',
} as const;

export const POWER_BUTTONS = [
  'power_shutdown',
  'power_restart',
  'power_sleep',
  'power_hibernate',
  'power_lock',
  'display_off',
] as const;
export type PowerButton = (typeof POWER_BUTTONS)[number];
