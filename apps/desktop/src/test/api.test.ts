import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, describe, expect, it } from 'vitest';
import { makeSnapshot } from '../../dev/mock-daemon';
import { app } from '../api/app';
import { DaemonHttpError, cmd, daemon } from '../api/daemon';
import { BridgeError } from '../api/tauri';

afterEach(() => {
  clearMocks();
});

interface Call {
  cmd: string;
  args: Record<string, unknown>;
}

function mock(handler: (c: Call) => unknown): Call[] {
  const calls: Call[] = [];
  mockIPC((command, args) => {
    const call = { cmd: command, args: (args ?? {}) as Record<string, unknown> };
    calls.push(call);
    return handler(call);
  });
  return calls;
}

describe('daemon API over ipc_request', () => {
  it('fetches and normalises the snapshot through the Rust command', async () => {
    const calls = mock(() => ({ status: 200, body: makeSnapshot('default') }));
    const snap = await daemon.snapshot();
    expect(snap.manifest.host.name).toBe('LEO-DESKTOP');
    expect(calls[0]).toEqual({
      cmd: 'ipc_request',
      args: { method: 'GET', path: '/ipc/snapshot', body: null },
    });
  });

  it('fills defaults when an older daemon omits the v0.6 fields', async () => {
    const raw = { ...makeSnapshot('default') } as Record<string, unknown>;
    const dropped = [
      'sensor_status',
      'session',
      'service',
      'updates',
      'cooling_policy',
      'mqtt_password_set',
    ];
    const trimmed = Object.fromEntries(Object.entries(raw).filter(([k]) => !dropped.includes(k)));
    mock(() => ({ status: 200, body: trimmed }));
    const snap = await daemon.snapshot();
    expect(snap.sensor_status).toEqual({});
    expect(snap.session).toBeNull();
    expect(snap.cooling_policy).toBeNull();
    expect(snap.updates.pending_count).toBeNull();
    expect(snap.service.agent_connected).toBe(false);
    expect(snap.mqtt_password_set).toBe(false);
  });

  it('rejects a payload that is not a snapshot', async () => {
    mock(() => ({ status: 200, body: { hello: 'world' } }));
    await expect(daemon.snapshot()).rejects.toMatchObject({ code: 'daemon_protocol' });
  });

  it('sends commands with the tagged JSON the daemon expects', async () => {
    const calls = mock(() => ({ status: 200, body: { ok: false, code: 'no_agent' } }));
    const r = await daemon.command(cmd.mute(true));
    expect(r).toEqual({ ok: false, code: 'no_agent' });
    expect(calls[0]?.args).toMatchObject({
      method: 'POST',
      path: '/ipc/command',
      body: { action: 'set_switch', entity_id: 'audio_mute', state: true },
    });
    expect(cmd.setCoolingPreset('gaming')).toEqual({
      action: 'set_cooling_preset',
      entity_id: 'cooling_preset',
      preset: 'gaming',
    });
    expect(cmd.light('light_ram', { color_temp_kelvin: 2700 })).toEqual({
      action: 'set_light',
      entity_id: 'light_ram',
      color_temp_kelvin: 2700,
    });
  });

  it('turns a 400 settings rejection into a typed error carrying the result code', async () => {
    mock(() => ({ status: 400, body: { ok: false, code: 'invalid_argument', message: 'bad id' } }));
    const err = await daemon.updateSettings({ scripts: [] }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(DaemonHttpError);
    expect((err as DaemonHttpError).status).toBe(400);
    expect((err as DaemonHttpError).code).toBe('invalid_argument');
  });

  it('url-encodes the client id on revoke', async () => {
    const calls = mock(() => ({ status: 200, body: makeSnapshot('default') }));
    await daemon.revokeClient('a b/c');
    expect(calls[0]?.args.path).toBe('/ipc/clients/a%20b%2Fc');
    expect(calls[0]?.args.method).toBe('DELETE');
  });

  it('maps Tauri-side failures to BridgeError codes (no sentences from Rust)', async () => {
    mock(() => {
      throw Object.assign(new Error('x'), { code: 'token_permission_denied' });
    });
    const err = await daemon.snapshot().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(BridgeError);
    expect((err as BridgeError).code).toBe('token_permission_denied');
  });

  it('falls back to an internal error for unknown rejections', async () => {
    mock(() => {
      throw new Error('boom');
    });
    await expect(daemon.snapshot()).rejects.toMatchObject({ code: 'internal' });
  });
});

describe('app API', () => {
  it('calls the documented commands with their arguments', async () => {
    const calls = mock(({ cmd: c }) => (c === 'app_info' ? { version: '0.6.0' } : null));
    await app.info();
    await app.setSettings({ close_to_tray: false, start_hidden: true, launch_at_login: false });
    await app.serviceInstallOrUpgrade('upgrade');
    await app.setWindowTheme('dark');
    await app.setTrayLabels('Panel', 'Show', 'Quit');
    expect(calls.map((c) => c.cmd)).toEqual([
      'app_info',
      'app_settings_set',
      'service_install_or_upgrade',
      'set_window_theme',
      'set_tray_labels',
    ]);
    expect(calls[1]?.args).toEqual({
      settings: { close_to_tray: false, start_hidden: true, launch_at_login: false },
    });
    expect(calls[2]?.args).toEqual({ action: 'upgrade' });
  });
});
