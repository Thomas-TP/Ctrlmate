export type SecurityMode = 'zero_kernel_safe' | 'deep_hardware_pawn_io';
export type PollingProfile = 'intelligent' | 'eco' | 'balanced' | 'realtime';
export type TemperatureUnit = 'celsius' | 'fahrenheit';
export type CoolingPreset = 'auto' | 'silent' | 'balanced' | 'performance' | 'full_speed';
export type LightColorMode = 'rgb' | 'color_temp';

export interface RgbColor {
  r: number;
  g: number;
  b: number;
}

export interface LightState {
  is_on: boolean;
  brightness: number;
  color_mode: LightColorMode;
  rgb: RgbColor;
  color_temp_kelvin: number;
}

export interface OsUpdateState {
  installed_version: string;
  latest_version: string;
  pending_count: number;
  pending_titles: string[];
  in_progress: boolean;
  reboot_required: boolean;
  last_checked?: string | null;
}

export type EntityValue =
  | { kind: 'float'; value: number }
  | { kind: 'int'; value: number }
  | { kind: 'bool'; value: boolean }
  | { kind: 'text'; value: string }
  | { kind: 'light'; value: LightState }
  | { kind: 'update'; value: OsUpdateState }
  | { kind: 'unavailable' };

export interface EntityState {
  id: string;
  value: EntityValue;
  attributes?: Record<string, unknown>;
  updated_at: string;
}

export interface HostDeviceInfo {
  device_id: string;
  name: string;
  manufacturer: string;
  model: string;
  os_name: string;
  os_version: string;
  sw_version: string;
  mac_address?: string | null;
  security_mode: SecurityMode;
}

export interface ChildDeviceInfo {
  id: string;
  name: string;
  category: string;
  manufacturer?: string | null;
  model?: string | null;
  hw_version?: string | null;
}

export interface EntityDescriptor {
  id: string;
  name: string;
  platform: string;
  child_device_id?: string | null;
  device_class?: string | null;
  unit_of_measurement?: string | null;
  options?: string[];
  exposed: boolean;
}

export interface DeviceManifest {
  protocol_version: number;
  host: HostDeviceInfo;
  child_devices: ChildDeviceInfo[];
  entities: EntityDescriptor[];
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

export interface PairingSessionInfo {
  active: boolean;
  pin?: string | null;
  expires_at?: string | null;
  cert_fingerprint: string;
  listen_port: number;
}

export interface PairedClientInfo {
  client_id: string;
  client_name: string;
  paired_at: string;
  connected: boolean;
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

export interface AllowlistedScript {
  id: string;
  name: string;
  executable: string;
  args: string[];
  working_dir?: string | null;
  enabled: boolean;
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
  deep_bridge_active: boolean;
}
