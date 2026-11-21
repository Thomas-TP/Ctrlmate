import type {
  AllowlistedScript,
  CoolingPreset,
  DaemonSnapshot,
  PollingProfile,
  RemotePermissions,
  RgbColor,
  SecurityMode,
  TemperatureUnit,
} from './types';

const DEFAULT_IPC_BASE = 'http://127.0.0.1:9488';

export function ipcHttpBase(): string {
  return DEFAULT_IPC_BASE;
}

export function ipcWsUrl(): string {
  return DEFAULT_IPC_BASE.replace(/^http/, 'ws') + '/ipc/ws';
}

export async function fetchSnapshot(): Promise<DaemonSnapshot> {
  const res = await fetch(`${ipcHttpBase()}/ipc/snapshot`);
  if (!res.ok) throw new Error(`Daemon IPC HTTP ${res.status}`);
  return res.json();
}

export async function startPairing(): Promise<DaemonSnapshot> {
  const res = await fetch(`${ipcHttpBase()}/ipc/pairing/start`, { method: 'POST' });
  if (!res.ok) throw new Error(`Start pairing HTTP ${res.status}`);
  return res.json();
}

export async function cancelPairing(): Promise<DaemonSnapshot> {
  const res = await fetch(`${ipcHttpBase()}/ipc/pairing/cancel`, { method: 'POST' });
  if (!res.ok) throw new Error(`Cancel pairing HTTP ${res.status}`);
  return res.json();
}

export async function revokeClient(clientId: string): Promise<DaemonSnapshot> {
  const res = await fetch(`${ipcHttpBase()}/ipc/clients/${encodeURIComponent(clientId)}`, {
    method: 'DELETE',
  });
  if (!res.ok) throw new Error(`Revoke HTTP ${res.status}`);
  return res.json();
}

export async function sendCommand(command: Record<string, unknown>): Promise<{ ok: boolean; message: string }> {
  const res = await fetch(`${ipcHttpBase()}/ipc/command`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(command),
  });
  return res.json();
}

export async function setLight(params: {
  entity_id: string;
  is_on?: boolean;
  brightness?: number;
  rgb?: RgbColor;
  color_temp_kelvin?: number;
  transition_ms?: number;
}) {
  return sendCommand({ action: 'set_light', ...params });
}

export async function setCoolingPreset(preset: CoolingPreset) {
  return sendCommand({ action: 'set_cooling_preset', entity_id: 'cooling_preset', preset });
}

export async function pressButton(entity_id: string) {
  return sendCommand({ action: 'press_button', entity_id });
}

export async function updateSettings(patch: {
  permissions?: RemotePermissions;
  polling_profile?: PollingProfile;
  temperature_unit?: TemperatureUnit;
  security_mode?: SecurityMode;
  scripts?: AllowlistedScript[];
}): Promise<DaemonSnapshot> {
  const res = await fetch(`${ipcHttpBase()}/ipc/settings`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(patch),
  });
  if (!res.ok) throw new Error(`Settings HTTP ${res.status}`);
  return res.json();
}

export function formatTemp(celsius: number | undefined, unit: TemperatureUnit): string {
  if (celsius === undefined || Number.isNaN(celsius)) return '—';
  if (unit === 'fahrenheit') {
    const f = (celsius * 9) / 5 + 32;
    return `${f.toFixed(1)} °F`;
  }
  return `${celsius.toFixed(1)} °C`;
}

export function kelvinToPreviewHex(kelvin: number): string {
  const temp = Math.max(2000, Math.min(6500, kelvin)) / 100;
  let r = 255;
  let g = 99.4708025861 * Math.log(temp) - 161.1195681661;
  let b = temp <= 19 ? 0 : 138.5177312231 * Math.log(temp - 10) - 305.0447927307;
  r = Math.max(0, Math.min(255, Math.round(r)));
  g = Math.max(0, Math.min(255, Math.round(g)));
  b = Math.max(0, Math.min(255, Math.round(b * 0.92)));
  return `#${r.toString(16).padStart(2, '0')}${g.toString(16).padStart(2, '0')}${b.toString(16).padStart(2, '0')}`;
}
