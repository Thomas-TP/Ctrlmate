import { describe, expect, it } from 'vitest';
import { makeSnapshot } from '../../dev/mock-daemon';
import { buildDiagnostics } from '../lib/diagnostics';
import { argsToText, isAbsolutePath, slugId, textToArgs, validateScripts } from '../lib/scripts';
import { lightZones, numberOf, orphanRgbDevices, readSensor } from '../lib/snapshot';
import { deriveAlerts, hottest, tempFill, temperatureRows } from '../lib/derive';
import type { AllowlistedScript } from '../types/protocol';

describe('honest sensors', () => {
  it('reports a missing CPU temperature with the daemon reason, not a number', () => {
    const snap = makeSnapshot('default');
    expect(numberOf(snap, 'cpu_0_temperature')).toBeNull();
    expect(readSensor(snap, 'cpu_0_temperature')).toEqual({
      value: null,
      available: false,
      reason: 'requires_deep_hardware',
    });
    expect(readSensor(snap, 'gpu_0_temperature')).toEqual({
      value: 46,
      available: true,
      reason: null,
    });
  });

  it('does not invent a reason when the daemon gave none', () => {
    const snap = makeSnapshot('default');
    expect(readSensor(snap, 'cooler_0_nonexistent').reason).toBeNull();
  });
});

describe('lighting zones', () => {
  it('come only from the manifest, master first', () => {
    const zones = lightZones(makeSnapshot('default'));
    expect(zones.map((z) => z.id)).toEqual([
      'light_global',
      'light_motherboard',
      'light_ram',
      'light_fans',
      'light_cooler',
    ]);
    expect(zones[0]?.isMaster).toBe(true);
    expect(zones.every((z) => z.controllable)).toBe(true);
  });

  it('lists devices without a zone as informational only', () => {
    const orphans = orphanRgbDevices(makeSnapshot('default'));
    expect(orphans.map((d) => d.name)).toEqual(['Samsung 990 PRO LED']);
    expect(orphans[0]?.reason).toBe('no_public_protocol');
  });

  it('marks a zone whose backing devices are all blocked as not controllable with the reason', () => {
    const snap = makeSnapshot('default');
    snap.rgb_devices = snap.rgb_devices.map((d) =>
      d.zone_id === 'light_ram'
        ? { ...d, controllable: false, reason: 'requires_deep_hardware' }
        : d,
    );
    const ram = lightZones(snap).find((z) => z.id === 'light_ram');
    expect(ram?.controllable).toBe(false);
    expect(ram?.reason).toBe('requires_deep_hardware');
  });

  it('shows no zone at all when the manifest has none', () => {
    const snap = makeSnapshot('default');
    snap.manifest.entities = snap.manifest.entities.filter((e) => e.platform !== 'light');
    expect(lightZones(snap)).toEqual([]);
  });
});

describe('derived dashboard data', () => {
  it('has no alert on a healthy default machine except the missing agent state', () => {
    const ids = deriveAlerts(makeSnapshot('default')).map((a) => a.id);
    expect(ids).not.toContain('failsafe');
  });

  it('puts the cooling safety mode first and as a danger', () => {
    const alerts = deriveAlerts(makeSnapshot('failsafe'));
    expect(alerts[0]).toMatchObject({ id: 'failsafe', tone: 'danger', sheet: 'cooling' });
  });

  it('lists CPU, GPU and liquid sensors first, honestly unavailable ones included', () => {
    const rows = temperatureRows(makeSnapshot('default'));
    expect(rows.slice(0, 3).map((r) => r.id)).toEqual([
      'cpu_0_temperature',
      'gpu_0_temperature',
      'cooler_0_liquid_temperature',
    ]);
    expect(rows[0]?.reading.value).toBeNull();
    expect(rows[1]?.reading.value).toBe(46);
  });

  it('reports the hottest readable sensor and maps temperatures to a bar fill', () => {
    expect(hottest(makeSnapshot('default'))).toBe(46);
    expect(tempFill(20)).toBe(0);
    expect(tempFill(60)).toBe(50);
    expect(tempFill(150)).toBe(100);
  });
});

describe('script allowlist validation (mirrors the daemon)', () => {
  const ok: AllowlistedScript = {
    id: 'backup',
    name: 'Backup',
    executable: 'C:\\Tools\\b.exe',
    args: [],
    working_dir: null,
    enabled: true,
  };
  it('accepts a valid list', () => {
    expect(
      validateScripts([
        ok,
        { ...ok, id: 'other', executable: '/usr/bin/true', working_dir: '/tmp' },
      ]).size,
    ).toBe(0);
  });
  it('flags bad ids, duplicates, relative paths and empty names', () => {
    const issues = validateScripts([
      { ...ok, id: 'Bad Id' },
      { ...ok, id: 'dup' },
      { ...ok, id: 'dup', executable: 'tool.exe', name: ' ', working_dir: 'rel' },
    ]);
    expect(issues.get(0)).toEqual(['id_invalid']);
    expect(issues.get(2)).toEqual([
      'id_duplicate',
      'name_empty',
      'exe_not_absolute',
      'dir_not_absolute',
    ]);
  });
  it('limits the list to 32 scripts', () => {
    const many = Array.from({ length: 33 }, (_, i) => ({ ...ok, id: `s${i}` }));
    expect(validateScripts(many).get(32)).toEqual(['too_many']);
  });
  it('recognises absolute paths of both platforms', () => {
    expect(
      ['C:\\a\\b.exe', 'C:/a/b', '\\\\srv\\share\\x', '/usr/bin/x'].every(isAbsolutePath),
    ).toBe(true);
    expect(['a.exe', './x', 'bin/x', ''].some(isAbsolutePath)).toBe(false);
  });
  it('builds ids and keeps arguments one per line', () => {
    expect(slugId('Sauvegarde été', new Set())).toBe('sauvegarde_ete');
    expect(slugId('Backup', new Set(['backup', 'backup_2']))).toBe('backup_3');
    expect(textToArgs(argsToText(['--target', 'D:\\My Backups']))).toEqual([
      '--target',
      'D:\\My Backups',
    ]);
  });
});

describe('diagnostics bundle', () => {
  it('contains no secret, address, host name or path', () => {
    const snap = makeSnapshot('default');
    snap.pairing = { ...snap.pairing, active: true, pin: '123456' };
    snap.mqtt = { ...snap.mqtt, host: 'broker.private.lan', username: 'leo', password: 'hunter2' };
    const text = buildDiagnostics({
      app: {
        version: '0.6.0',
        platform: 'windows',
        arch: 'x86_64',
        debug: false,
        ipc_port: 9488,
        token_source: 'system',
        update_channel: 'win',
      },
      connection: { status: 'connected', attempt: 0 },
      snapshot: snap,
      locale: 'en',
      now: new Date('2026-11-25T10:00:00Z'),
    });
    for (const secret of [
      '123456',
      'broker.private.lan',
      'leo',
      'hunter2',
      'LEO-DESKTOP',
      '00:11:22:33:44:55',
      'mock-device',
      'C:\\\\Tools',
      'Home Assistant (',
      snap.pairing.cert_fingerprint,
    ]) {
      expect(text, secret).not.toContain(secret);
    }
    const parsed = JSON.parse(text) as {
      app: { token_source: string };
      daemon: { counts: { entities: number } };
    };
    expect(parsed.app.token_source).toBe('system');
    expect(parsed.daemon.counts.entities).toBe(snap.manifest.entities.length);
  });
});
