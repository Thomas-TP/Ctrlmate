// Dev-only mock of the Tauri side (`src-tauri`): installs a fake IPC layer so the real frontend
// code runs unchanged in a plain browser. Reached only through a dynamic import behind
// `import.meta.env.DEV` (see src/main.tsx).

import { emit } from '@tauri-apps/api/event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { MockDaemon, type Scenario } from './mock-daemon';

const SCENARIOS: readonly Scenario[] = [
  'default',
  'deep',
  'failsafe',
  'noagent',
  'offline',
  'unauthorized',
  'linux-denied',
];

interface Conn {
  status: 'connecting' | 'connected' | 'unauthorized' | 'offline';
  reason?: string | null;
  attempt: number;
  retry_in_ms?: number | null;
}

/** Same shape as the Rust `IpcError`: an object with a stable `code`. */
const bridgeError = (code: string, detail?: string) =>
  Object.assign(new Error(code), { code, detail });

export function installMockBridge(param = '1'): void {
  const scenario: Scenario = (SCENARIOS as readonly string[]).includes(param)
    ? (param as Scenario)
    : 'default';
  const blocked =
    scenario === 'offline' || scenario === 'unauthorized' || scenario === 'linux-denied';
  let conn: Conn = { status: 'connecting', attempt: 0 };
  const setConn = (c: Conn) => {
    conn = c;
    void emit('daemon://connection', c);
  };
  const failure = (): Conn =>
    scenario === 'offline'
      ? { status: 'offline', reason: 'daemon_offline', attempt: 2, retry_in_ms: 4000 }
      : scenario === 'linux-denied'
        ? {
            status: 'unauthorized',
            reason: 'token_permission_denied',
            attempt: 1,
            retry_in_ms: 2000,
          }
        : { status: 'unauthorized', reason: 'unauthorized', attempt: 1, retry_in_ms: 2000 };

  const daemon = new MockDaemon(scenario, (snap) => {
    if (!blocked) void emit('daemon://event', { type: 'snapshot', ...snap });
  });
  let settings = { close_to_tray: true, start_hidden: false };
  let outdatedService = false;

  mockIPC(
    (cmd, args) => {
      const a = (args ?? {}) as Record<string, unknown>;
      switch (cmd) {
        case 'app_info':
          return {
            version: '0.6.0',
            platform: 'windows',
            arch: 'x86_64',
            debug: true,
            ipc_port: 9488,
            token_source: blocked ? null : 'system',
            update_channel: 'win',
          };
        case 'connection_status':
          return conn;
        case 'daemon_reconnect':
          if (blocked) {
            setConn({ status: 'connecting', attempt: 0 });
            setTimeout(() => setConn(failure()), 700);
          }
          return null;
        case 'ipc_request': {
          if (blocked)
            throw bridgeError(
              scenario === 'offline'
                ? 'daemon_offline'
                : scenario === 'linux-denied'
                  ? 'token_permission_denied'
                  : 'unauthorized',
            );
          return daemon.handle(String(a.method), String(a.path), a.body ?? null);
        }
        case 'service_status':
          return {
            platform: 'windows',
            app_version: '0.6.0',
            daemon_version: blocked ? null : outdatedService ? '0.5.2' : '0.6.0',
            running_as_service: blocked ? null : true,
            agent_connected: blocked ? null : daemon.snapshot.service.agent_connected,
            state: blocked ? 'offline' : outdatedService ? 'outdated' : 'current',
            installer_available: true,
            agent_binary_available: true,
          };
        case 'service_install_or_upgrade':
          outdatedService = false;
          return new Promise((r) =>
            setTimeout(() => r({ status: 'completed', exit_code: 0 }), 900),
          );
        case 'agent_install_autostart':
          return { ok: true, exit_code: 0 };
        case 'app_update_check':
          return new Promise((r) =>
            setTimeout(
              () =>
                r({
                  state: 'available',
                  channel: 'win',
                  current_version: '0.6.0',
                  latest_version: '0.6.1',
                  detail: null,
                }),
              600,
            ),
          );
        case 'app_update_install': {
          let p = 0;
          const id = setInterval(() => {
            p += 20;
            void emit('app-update://progress', Math.min(100, p));
            if (p >= 100) clearInterval(id);
          }, 300);
          return null;
        }
        case 'app_settings_get':
          return settings;
        case 'app_settings_set':
          settings = a.settings as typeof settings;
          return null;
        case 'set_tray_labels':
        case 'set_window_theme':
        case 'window_hide':
        case 'app_quit':
          return null;
        default:
          throw bridgeError('internal', `mock: unknown command ${cmd}`);
      }
    },
    { shouldMockEvents: true },
  );

  // Simulate the first connection, then the sensor drift.
  setTimeout(() => {
    if (blocked) {
      setConn(failure());
      return;
    }
    setConn({ status: 'connected', attempt: 0 });
    void emit('daemon://event', { type: 'auth_ok' });
    void emit('daemon://event', { type: 'snapshot', ...daemon.snapshot });
    // Seed some history so sparklines are not empty on first paint.
    for (let i = 0; i < 24; i++) daemon.tick();
    setInterval(() => daemon.tick(), 1600);
  }, 450);
}
