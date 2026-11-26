import { call, on, EVENT_CONNECTION, EVENT_DAEMON, EVENT_UPDATE_PROGRESS } from './tauri';
import { normalizeSnapshot } from '../lib/snapshot';
import type { DaemonSnapshot } from '../types/protocol';

export type ConnState = 'connecting' | 'connected' | 'unauthorized' | 'offline';

export interface ConnectionStatus {
  status: ConnState;
  /** Error code explaining `unauthorized` / `offline` (translate with `connection.reason.*`). */
  reason?: string | null;
  attempt: number;
  retry_in_ms?: number | null;
}

export interface AppInfo {
  version: string;
  platform: 'windows' | 'linux' | 'other';
  arch: string;
  debug: boolean;
  ipc_port: number;
  token_source: 'env' | 'user' | 'system' | 'dev' | null;
  update_channel: string;
}

export type ServiceState = 'offline' | 'not_installed' | 'current' | 'outdated' | 'newer';

export interface ServiceStatus {
  platform: 'windows' | 'linux' | 'other';
  app_version: string;
  daemon_version: string | null;
  running_as_service: boolean | null;
  agent_connected: boolean | null;
  state: ServiceState;
  installer_available: boolean;
  agent_binary_available: boolean;
}

export type ElevatedStatus = 'completed' | 'cancelled' | 'failed' | 'manual';
export interface ElevatedOutcome {
  status: ElevatedStatus;
  exit_code: number | null;
}

export interface AutostartOutcome {
  ok: boolean;
  exit_code: number | null;
}

export type AppUpdateState = 'unsupported' | 'up_to_date' | 'available' | 'error';
export interface AppUpdateStatus {
  state: AppUpdateState;
  channel: string;
  current_version: string;
  latest_version: string | null;
  detail: string | null;
}

export interface AppSettings {
  close_to_tray: boolean;
  start_hidden: boolean;
  launch_at_login: boolean;
}

export type DaemonEvent = { type: 'auth_ok' } | { type: 'snapshot'; snapshot: DaemonSnapshot };

export const app = {
  info: () => call<AppInfo>('app_info'),
  connectionStatus: () => call<ConnectionStatus>('connection_status'),
  reconnect: () => call<null>('daemon_reconnect'),
  serviceStatus: () => call<ServiceStatus>('service_status'),
  serviceInstallOrUpgrade: (action: 'install' | 'upgrade') =>
    call<ElevatedOutcome>('service_install_or_upgrade', { action }),
  agentInstallAutostart: () => call<AutostartOutcome>('agent_install_autostart'),
  updateCheck: () => call<AppUpdateStatus>('app_update_check'),
  updateInstall: () => call<null>('app_update_install'),
  settings: () => call<AppSettings>('app_settings_get'),
  setSettings: (settings: AppSettings) => call<null>('app_settings_set', { settings }),
  setTrayLabels: (show: string, quit: string) => call<null>('set_tray_labels', { show, quit }),
  setWindowTheme: (theme: 'light' | 'dark' | null) => call<null>('set_window_theme', { theme }),
  hide: () => call<null>('window_hide'),
  quit: () => call<null>('app_quit'),
};

/** Daemon events (`daemon://event`): snapshots are validated and normalised before delivery. */
export function listenDaemon(handler: (e: DaemonEvent) => void) {
  return on<unknown>(EVENT_DAEMON, (payload) => {
    if (typeof payload !== 'object' || payload === null) return;
    const type = (payload as { type?: unknown }).type;
    if (type === 'auth_ok') handler({ type: 'auth_ok' });
    else if (type === 'snapshot') {
      const snapshot = normalizeSnapshot(payload);
      if (snapshot) handler({ type: 'snapshot', snapshot });
    }
  });
}

export function listenConnection(handler: (s: ConnectionStatus) => void) {
  return on<ConnectionStatus>(EVENT_CONNECTION, handler);
}

export function listenUpdateProgress(handler: (percent: number) => void) {
  return on<number>(EVENT_UPDATE_PROGRESS, handler);
}
