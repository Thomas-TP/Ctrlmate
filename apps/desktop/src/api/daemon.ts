import { normalizeSnapshot } from '../lib/snapshot';
import type {
  CommandResult,
  ControlCommand,
  CoolingPreset,
  DaemonSnapshot,
  RgbColor,
  SettingsUpdate,
} from '../types/protocol';
import { BridgeError, call } from './tauri';

interface IpcResponse {
  status: number;
  body: unknown;
}

/** The daemon answered with an HTTP error status (e.g. 400 + CommandResult, 404). */
export class DaemonHttpError extends Error {
  readonly status: number;
  /** The `CommandResult` of the rejection when the body carried one. */
  readonly result: CommandResult | null;
  constructor(status: number, result: CommandResult | null) {
    super(`HTTP ${status}`);
    this.name = 'DaemonHttpError';
    this.status = status;
    this.result = result;
  }
  /** Stable code to translate (`invalid_argument` for 400/404 without a body). */
  get code(): string {
    return (
      this.result?.code ??
      (this.status === 404 || this.status === 400 ? 'invalid_argument' : 'internal')
    );
  }
}

export function isCommandResult(v: unknown): v is CommandResult {
  return (
    typeof v === 'object' &&
    v !== null &&
    typeof (v as CommandResult).ok === 'boolean' &&
    typeof (v as CommandResult).code === 'string'
  );
}

async function ipc(method: string, path: string, body?: unknown): Promise<IpcResponse> {
  return call<IpcResponse>('ipc_request', { method, path, body: body ?? null });
}

function expectSnapshot(res: IpcResponse): DaemonSnapshot {
  if (res.status >= 200 && res.status < 300) {
    const snap = normalizeSnapshot(res.body);
    if (!snap) throw new BridgeError('daemon_protocol');
    return snap;
  }
  throw new DaemonHttpError(res.status, isCommandResult(res.body) ? res.body : null);
}

export const daemon = {
  snapshot: async (): Promise<DaemonSnapshot> => expectSnapshot(await ipc('GET', '/ipc/snapshot')),
  startPairing: async (): Promise<DaemonSnapshot> =>
    expectSnapshot(await ipc('POST', '/ipc/pairing/start')),
  cancelPairing: async (): Promise<DaemonSnapshot> =>
    expectSnapshot(await ipc('POST', '/ipc/pairing/cancel')),
  revokeClient: async (clientId: string): Promise<DaemonSnapshot> =>
    expectSnapshot(await ipc('DELETE', `/ipc/clients/${encodeURIComponent(clientId)}`)),
  rescanHardware: async (): Promise<DaemonSnapshot> =>
    expectSnapshot(await ipc('POST', '/ipc/hardware/rescan')),
  /** Applies a partial settings update. A rejected update throws `DaemonHttpError` (400 + code). */
  updateSettings: async (update: SettingsUpdate): Promise<DaemonSnapshot> =>
    expectSnapshot(await ipc('PUT', '/ipc/settings', update)),
  /** Sends a control command; the HTTP status is always 200, the outcome is in `code`. */
  command: async (cmd: ControlCommand): Promise<CommandResult> => {
    const res = await ipc('POST', '/ipc/command', cmd);
    if (isCommandResult(res.body)) return res.body;
    throw new DaemonHttpError(res.status, null);
  },
};

// Typed builders for the commands the UI sends -------------------------------------------------

export const cmd = {
  setCoolingPreset: (preset: CoolingPreset): ControlCommand => ({
    action: 'set_cooling_preset',
    entity_id: 'cooling_preset',
    preset,
  }),
  light: (
    entityId: string,
    patch: {
      is_on?: boolean;
      brightness?: number;
      rgb?: RgbColor;
      color_temp_kelvin?: number;
      transition_ms?: number;
    },
  ): ControlCommand => ({ action: 'set_light', entity_id: entityId, ...patch }),
  press: (entityId: string): ControlCommand => ({ action: 'press_button', entity_id: entityId }),
  volume: (percent: number): ControlCommand => ({
    action: 'set_number',
    entity_id: 'master_volume',
    value: percent,
  }),
  mute: (state: boolean): ControlCommand => ({
    action: 'set_switch',
    entity_id: 'audio_mute',
    state,
  }),
  audioOutput: (option: string): ControlCommand => ({
    action: 'set_select',
    entity_id: 'audio_output',
    option,
  }),
  notify: (title: string, message: string): ControlCommand => ({
    action: 'send_notification',
    title,
    message,
  }),
  checkOsUpdates: (): ControlCommand => ({ action: 'check_os_updates' }),
  installOsUpdates: (): ControlCommand => ({ action: 'install_os_updates' }),
};
