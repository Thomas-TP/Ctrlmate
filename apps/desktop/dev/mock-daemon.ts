// Dev-only mock of ctrlmated (loopback IPC). Used by `?mock=1` in `bun run dev` and by component
// tests. It is never imported from production code paths: `src/main.tsx` reaches it through a
// dynamic import guarded by `import.meta.env.DEV`, so bundlers drop it from release builds.
//
// Data models the owner's PC: Ryzen 9 7900X3D, RTX 5070, NZXT Kraken 360, G.Skill Trident Z5,
// Samsung 990 PRO, Gigabyte board, on Windows 11 in the default driver-free mode (no CPU
// temperature).

import type {
  AllowlistedScript,
  CommandResult,
  ControlCommand,
  CoolingPolicy,
  DaemonSnapshot,
  EntityDescriptor,
  EntityState,
  LightState,
  SettingsUpdate,
} from '../src/types/protocol';

export type Scenario =
  'default' | 'deep' | 'failsafe' | 'noagent' | 'offline' | 'unauthorized' | 'linux-denied';

const now = () => new Date().toISOString();
const state = (id: string, value: EntityState['value']): EntityState => ({
  id,
  value,
  updated_at: now(),
});
const num = (id: string, v: number): EntityState => state(id, { kind: 'float', value: v });

const CURVES: CoolingPolicy['presets'] = [
  {
    preset: 'auto',
    pump_floor: 60,
    fan_curve: [
      [30, 30],
      [50, 40],
      [62, 55],
      [72, 75],
      [82, 95],
      [90, 100],
    ].map(([t, d]) => ({ temp_c: t as number, duty: d as number })),
  },
  {
    preset: 'silent',
    pump_floor: 50,
    fan_curve: [
      [30, 25],
      [50, 30],
      [65, 40],
      [75, 60],
      [85, 85],
      [92, 100],
    ].map(([t, d]) => ({ temp_c: t as number, duty: d as number })),
  },
  {
    preset: 'balanced',
    pump_floor: 60,
    fan_curve: [
      [30, 30],
      [50, 40],
      [60, 55],
      [70, 70],
      [80, 90],
      [88, 100],
    ].map(([t, d]) => ({ temp_c: t as number, duty: d as number })),
  },
  {
    preset: 'performance',
    pump_floor: 80,
    fan_curve: [
      [30, 40],
      [45, 55],
      [55, 70],
      [65, 85],
      [75, 100],
    ].map(([t, d]) => ({ temp_c: t as number, duty: d as number })),
  },
  {
    preset: 'gaming',
    pump_floor: 90,
    fan_curve: [
      [30, 45],
      [50, 60],
      [60, 75],
      [70, 90],
      [78, 100],
    ].map(([t, d]) => ({ temp_c: t as number, duty: d as number })),
  },
];

const light = (over: Partial<LightState> = {}): LightState => ({
  is_on: true,
  brightness: 204,
  color_mode: 'color_temp',
  rgb: { r: 255, g: 224, b: 189 },
  color_temp_kelvin: 3200,
  ...over,
});

const lightDescriptor = (
  id: string,
  name: string,
  child: string | null,
  leds: number,
  master = false,
): EntityDescriptor => ({
  id,
  name,
  platform: 'light',
  child_device_id: child,
  device_class: null,
  state_class: null,
  unit_of_measurement: null,
  icon: null,
  entity_category: null,
  suggested_display_precision: null,
  light_caps: {
    supports_rgb: true,
    supports_color_temp: true,
    min_color_temp_kelvin: 2000,
    max_color_temp_kelvin: 6500,
    supports_transition: true,
    is_master_zone: master,
    led_count: leds,
  },
});

const descriptor = (
  id: string,
  name: string,
  platform: EntityDescriptor['platform'],
  child: string | null,
  unit: string | null = null,
): EntityDescriptor => ({
  id,
  name,
  platform,
  child_device_id: child,
  device_class: null,
  state_class: null,
  unit_of_measurement: unit,
  icon: null,
  entity_category: null,
  suggested_display_precision: null,
});

export function makeSnapshot(scenario: Scenario = 'default'): DaemonSnapshot {
  const deep = scenario === 'deep' || scenario === 'failsafe';
  const agent = scenario !== 'noagent';
  const entities: EntityDescriptor[] = [
    descriptor('cpu_0_temperature', 'Temperature', 'sensor', 'cpu_0', '\u00b0C'),
    descriptor('cpu_0_load', 'Load', 'sensor', 'cpu_0', '%'),
    descriptor('gpu_0_temperature', 'Temperature', 'sensor', 'gpu_0', '\u00b0C'),
    descriptor('gpu_0_load', 'Load', 'sensor', 'gpu_0', '%'),
    descriptor(
      'cooler_0_liquid_temperature',
      'Liquid temperature',
      'sensor',
      'cooler_0',
      '\u00b0C',
    ),
    descriptor('cooler_0_fan_rpm', 'Fan speed', 'sensor', 'cooler_0', 'rpm'),
    descriptor('cooler_0_fan_duty', 'Fan duty', 'sensor', 'cooler_0', '%'),
    descriptor('cooler_0_pump_rpm', 'Pump speed', 'sensor', 'cooler_0', 'rpm'),
    descriptor('cooler_0_pump_duty', 'Pump duty', 'sensor', 'cooler_0', '%'),
    lightDescriptor('light_global', 'All lights', null, 1, true),
    lightDescriptor('light_motherboard', 'Motherboard', 'motherboard_0', 12),
    lightDescriptor('light_ram', 'Memory', 'memory_0', 20),
    lightDescriptor('light_fans', 'Fans', 'cooler_0', 36),
    lightDescriptor('light_cooler', 'Pump head', 'cooler_0', 24),
    descriptor('cooling_preset', 'Cooling preset', 'select', null),
    descriptor('master_volume', 'Volume', 'number', null, '%'),
    descriptor('audio_mute', 'Mute', 'switch', null),
    descriptor('audio_output', 'Audio output', 'select', null),
    descriptor('os_updates', 'System updates', 'update', null),
    ...[
      'power_shutdown',
      'power_restart',
      'power_sleep',
      'power_hibernate',
      'power_lock',
      'display_off',
      'script_backup',
    ].map((id) => descriptor(id, id, 'button', null)),
  ];
  const states: EntityState[] = [
    num('cpu_0_load', 18),
    num('gpu_0_temperature', 46),
    num('gpu_0_load', 9),
    num('cooler_0_liquid_temperature', 31.5),
    num('cooler_0_fan_rpm', 780),
    num('cooler_0_fan_duty', 38),
    num('cooler_0_pump_rpm', 2410),
    num('cooler_0_pump_duty', 60),
    state('light_global', { kind: 'light', value: light() }),
    state('light_motherboard', {
      kind: 'light',
      value: light({ color_temp_kelvin: 2700, brightness: 160 }),
    }),
    state('light_ram', { kind: 'light', value: light({ color_temp_kelvin: 3500 }) }),
    state('light_fans', {
      kind: 'light',
      value: light({ color_mode: 'rgb', rgb: { r: 70, g: 110, b: 240 }, brightness: 128 }),
    }),
    state('light_cooler', { kind: 'light', value: light({ is_on: false }) }),
  ];
  if (deep) states.push(num('cpu_0_temperature', scenario === 'failsafe' ? 94 : 52));

  const policy: CoolingPolicy = {
    presets: CURVES,
    thresholds: {
      cpu_critical_c: 93,
      gpu_critical_c: 88,
      liquid_critical_c: 55,
      hysteresis_c: 5,
      pump_floor_c: 35,
    },
    requested_preset: 'auto',
    effective_preset: 'auto',
    state: scenario === 'failsafe' ? 'failsafe' : 'normal',
    reason: scenario === 'failsafe' ? 'temperature' : null,
  };
  const scripts: AllowlistedScript[] = [
    {
      id: 'backup',
      name: 'Backup photos',
      executable: 'C:\\Tools\\backup.exe',
      args: ['--incremental', '--target', 'D:\\Backups'],
      working_dir: null,
      enabled: true,
    },
  ];
  return {
    manifest: {
      protocol_version: 1,
      host: {
        device_id: 'mock-device',
        name: 'LEO-DESKTOP',
        manufacturer: 'Gigabyte Technology',
        model: 'X670 AORUS ELITE AX',
        os_name: 'Windows 11 Pro',
        os_version: '10.0.26340',
        sw_version: '0.6.0',
        mac_address: '00:11:22:33:44:55',
        security_mode: deep ? 'deep_hardware_pawn_io' : 'zero_kernel_safe',
      },
      child_devices: [
        {
          id: 'cpu_0',
          name: 'AMD Ryzen 9 7900X3D',
          category: 'cpu',
          manufacturer: 'AMD',
          model: null,
          hw_version: null,
        },
        {
          id: 'gpu_0',
          name: 'NVIDIA GeForce RTX 5070',
          category: 'gpu',
          manufacturer: 'NVIDIA',
          model: null,
          hw_version: null,
        },
        {
          id: 'cooler_0',
          name: 'NZXT Kraken 360',
          category: 'cooler',
          manufacturer: 'NZXT',
          model: null,
          hw_version: null,
        },
        {
          id: 'memory_0',
          name: 'G.Skill Trident Z5 RGB',
          category: 'memory',
          manufacturer: 'G.Skill',
          model: null,
          hw_version: null,
        },
        {
          id: 'storage_nvme0',
          name: 'Samsung 990 PRO',
          category: 'storage',
          manufacturer: 'Samsung',
          model: null,
          hw_version: null,
        },
        {
          id: 'motherboard_0',
          name: 'Gigabyte X670 AORUS',
          category: 'motherboard',
          manufacturer: 'Gigabyte',
          model: null,
          hw_version: null,
        },
      ],
      entities,
    },
    states,
    conflicts: [],
    hid_devices: [],
    pairing: {
      active: false,
      pin: null,
      expires_at: null,
      cert_fingerprint: 'A3F1C2D4E5B60718293A4B5C6D7E8F90112233445566778899AABBCCDDEEFF00',
      listen_port: 9487,
    },
    paired_clients: [
      {
        client_id: 'c1',
        client_name: 'Home Assistant (homeassistant.local)',
        paired_at: '2026-11-02T18:20:00Z',
        connected: true,
        connections: 1,
      },
      {
        client_id: 'c2',
        client_name: 'Home Assistant (test instance)',
        paired_at: '2026-11-12T09:05:00Z',
        connected: false,
        connections: 0,
      },
    ],
    permissions: {
      telemetry: true,
      lighting: true,
      cooling: true,
      audio_and_media: true,
      power_control: true,
      os_updates: true,
      notifications: true,
      custom_scripts: false,
    },
    polling_profile: 'intelligent',
    temperature_unit: 'celsius',
    scripts,
    thermal_failsafe_active: scenario === 'failsafe',
    rgb_backends: ['gigabyte_ite', 'gskill_ene_smbus', 'nzxt_rgb', 'openrgb'],
    rgb_devices: [
      {
        id: 'mb',
        name: 'Gigabyte RGB Fusion 2',
        zone_id: 'light_motherboard',
        transport: 'USB HID',
        controllable: true,
        reason: 'ok',
      },
      {
        id: 'ram',
        name: 'G.Skill Trident Z5 RGB',
        zone_id: 'light_ram',
        transport: 'SMBus',
        controllable: true,
        reason: 'native_ene_smbus',
      },
      {
        id: 'fans',
        name: 'NZXT RGB Controller (x3)',
        zone_id: 'light_fans',
        transport: 'USB HID',
        controllable: true,
        reason: 'ok',
      },
      {
        id: 'kraken',
        name: 'NZXT Kraken 360',
        zone_id: 'light_cooler',
        transport: 'OpenRGB',
        controllable: true,
        reason: 'via_openrgb',
      },
      {
        id: 'nvme',
        name: 'Samsung 990 PRO LED',
        zone_id: 'light_storage',
        transport: 'NVMe',
        controllable: false,
        reason: 'no_public_protocol',
      },
    ],
    deep_bridge_active: deep,
    mqtt: {
      enabled: false,
      host: 'homeassistant.local',
      port: 1883,
      username: null,
      password: null,
      use_tls: false,
      discovery_prefix: 'homeassistant',
    },
    mqtt_connected: false,
    mqtt_password_set: false,
    cooling_preset: 'auto',
    cooling_policy: policy,
    sensor_status: {
      cpu_0_temperature: deep
        ? { available: true }
        : { available: false, reason: 'requires_deep_hardware' },
      gpu_0_temperature: { available: true },
      cooler_0_liquid_temperature: { available: true },
    },
    session: agent
      ? {
          foreground_app: { process_name: 'Code.exe', window_title: 'Ctrlmate' },
          gaming_mode: false,
          locked: false,
          idle_seconds: 12,
          mic_in_use: false,
          webcam_in_use: null,
          audio: {
            volume: 32,
            muted: false,
            output_id: 'out-1',
            outputs: [
              { id: 'out-1', name: 'Speakers (Realtek)' },
              { id: 'out-2', name: 'Headphones (USB DAC)' },
            ],
          },
        }
      : null,
    service: {
      running_as_service: true,
      version: '0.6.0',
      protocol_version: 1,
      session_type: 'service',
      agent_connected: agent,
      in_process_session: false,
      data_dir_kind: 'system',
      agent_version: agent ? '0.6.0' : null,
      agent_session_id: agent ? 2 : null,
    },
    updates: {
      checked: true,
      last_checked: new Date(Date.now() - 25 * 60_000).toISOString(),
      pending_count: 2,
      pending: [
        { title: '2026-11 Cumulative Update for Windows 11 (KB5050009)', kb: 'KB5050009' },
        { title: 'Windows Security platform update (KB5007651)', kb: 'KB5007651' },
      ],
      titles: [
        '2026-11 Cumulative Update for Windows 11 (KB5050009)',
        'Windows Security platform update (KB5007651)',
      ],
      kb_ids: ['KB5050009', 'KB5007651'],
      reboot_required: false,
      in_progress: false,
      error: null,
      installed_version: '24H2 (26340.1)',
    },
  };
}

export interface MockResponse {
  status: number;
  body: unknown;
}

const result = (r: CommandResult): MockResponse => ({ status: 200, body: r });
const ok = (message?: string): CommandResult =>
  message ? { ok: true, code: 'ok', message } : { ok: true, code: 'ok' };
const err = (code: string, message?: string): CommandResult => ({
  ok: false,
  code,
  ...(message ? { message } : {}),
});

/** Stateful fake of the IPC routes. `onChange` is called after every mutation. */
export class MockDaemon {
  snapshot: DaemonSnapshot;
  private phase = 0;

  constructor(
    readonly scenario: Scenario,
    private readonly onChange: (s: DaemonSnapshot) => void,
  ) {
    this.snapshot = makeSnapshot(scenario);
  }

  private upsert(id: string, value: EntityState['value']) {
    const i = this.snapshot.states.findIndex((s) => s.id === id);
    const next = state(id, value);
    if (i >= 0) this.snapshot.states[i] = next;
    else this.snapshot.states.push(next);
  }

  private num(id: string): number | null {
    const v = this.snapshot.states.find((s) => s.id === id)?.value;
    return v && v.kind === 'float' ? v.value : null;
  }

  private emit() {
    this.snapshot = { ...this.snapshot, states: [...this.snapshot.states] };
    this.onChange(this.snapshot);
  }

  /** Slow sinusoidal drift so charts have something to draw. */
  tick() {
    this.phase += 0.35;
    const wave = (base: number, amp: number, shift = 0) =>
      base + Math.sin(this.phase + shift) * amp + (Math.random() - 0.5) * amp * 0.3;
    const policy = this.snapshot.cooling_policy;
    const cpuT = this.num('cpu_0_temperature');
    if (cpuT !== null && policy?.state !== 'failsafe')
      this.upsert('cpu_0_temperature', { kind: 'float', value: Math.round(wave(58, 9) * 10) / 10 });
    this.upsert('cpu_0_load', { kind: 'float', value: Math.max(2, Math.round(wave(24, 18, 1))) });
    this.upsert('gpu_0_temperature', {
      kind: 'float',
      value: Math.round(wave(47, 4, 2) * 10) / 10,
    });
    this.upsert('gpu_0_load', { kind: 'float', value: Math.max(1, Math.round(wave(14, 10, 3))) });
    this.upsert('cooler_0_liquid_temperature', {
      kind: 'float',
      value: Math.round(wave(32, 1.5, 4) * 10) / 10,
    });
    const fan = policy?.state === 'failsafe' ? 100 : Math.round(wave(42, 10, 5));
    this.upsert('cooler_0_fan_duty', { kind: 'float', value: fan });
    this.upsert('cooler_0_fan_rpm', { kind: 'float', value: Math.round(fan * 20.5) });
    const pump =
      policy?.state === 'failsafe'
        ? 100
        : (policy?.presets.find((p) => p.preset === policy.effective_preset)?.pump_floor ?? 60);
    this.upsert('cooler_0_pump_duty', { kind: 'float', value: pump });
    this.upsert('cooler_0_pump_rpm', { kind: 'float', value: Math.round(pump * 40) });
    if (this.snapshot.session)
      this.snapshot.session = {
        ...this.snapshot.session,
        idle_seconds: this.snapshot.session.idle_seconds + 2,
      };
    this.emit();
  }

  handle(method: string, path: string, body: unknown): MockResponse {
    const s = this.snapshot;
    const m = method.toUpperCase();
    if (m === 'GET' && path === '/ipc/snapshot') return { status: 200, body: s };
    if (m === 'POST' && path === '/ipc/pairing/start') {
      s.pairing = {
        ...s.pairing,
        active: true,
        pin: '482913',
        expires_at: new Date(Date.now() + 5 * 60_000).toISOString(),
      };
      this.emit();
      return { status: 200, body: this.snapshot };
    }
    if (m === 'POST' && path === '/ipc/pairing/cancel') {
      s.pairing = { ...s.pairing, active: false, pin: null, expires_at: null };
      this.emit();
      return { status: 200, body: this.snapshot };
    }
    if (m === 'POST' && path === '/ipc/hardware/rescan') {
      this.emit();
      return { status: 200, body: this.snapshot };
    }
    if (m === 'DELETE' && path.startsWith('/ipc/clients/')) {
      const id = decodeURIComponent(path.slice('/ipc/clients/'.length));
      if (!s.paired_clients.some((c) => c.client_id === id))
        return { status: 404, body: err('invalid_argument', 'unknown client') };
      s.paired_clients = s.paired_clients.filter((c) => c.client_id !== id);
      this.emit();
      return { status: 200, body: this.snapshot };
    }
    if (m === 'PUT' && path === '/ipc/settings') return this.settings(body as SettingsUpdate);
    if (m === 'POST' && path === '/ipc/command')
      return result(this.command(body as ControlCommand));
    return { status: 404, body: err('not_supported') };
  }

  private settings(u: SettingsUpdate): MockResponse {
    const s = this.snapshot;
    if (u.scripts) {
      const bad = u.scripts.some(
        (x) => !/^[a-z0-9_-]{1,32}$/.test(x.id) || !/^([A-Za-z]:[\\/]|\/)/.test(x.executable),
      );
      if (bad) return { status: 400, body: err('invalid_argument', 'invalid script') };
      s.scripts = u.scripts;
    }
    if (u.permissions) s.permissions = u.permissions;
    if (u.polling_profile) s.polling_profile = u.polling_profile;
    if (u.temperature_unit) s.temperature_unit = u.temperature_unit;
    if (u.security_mode)
      s.manifest = { ...s.manifest, host: { ...s.manifest.host, security_mode: u.security_mode } };
    if (u.mqtt) {
      const { password, ...rest } = u.mqtt;
      s.mqtt = { ...rest, password: null };
      if (password === '') s.mqtt_password_set = false;
      else if (password) s.mqtt_password_set = true;
      s.mqtt_connected = u.mqtt.enabled;
    }
    this.emit();
    return { status: 200, body: this.snapshot };
  }

  private command(c: ControlCommand): CommandResult {
    const s = this.snapshot;
    const needsAgent = () => (s.service.agent_connected ? null : err('no_agent'));
    switch (c.action) {
      case 'set_light': {
        const cur = s.states.find((x) => x.id === c.entity_id)?.value;
        if (cur?.kind !== 'light') return err('invalid_argument', 'unknown lighting entity');
        const next: LightState = { ...cur.value };
        if (c.is_on !== undefined && c.is_on !== null) next.is_on = c.is_on;
        if (c.brightness !== undefined && c.brightness !== null) next.brightness = c.brightness;
        if (c.rgb) {
          next.rgb = c.rgb;
          next.color_mode = 'rgb';
        }
        if (c.color_temp_kelvin) {
          next.color_temp_kelvin = c.color_temp_kelvin;
          next.color_mode = 'color_temp';
        }
        this.upsert(c.entity_id, { kind: 'light', value: next });
        this.emit();
        return ok();
      }
      case 'set_cooling_preset': {
        s.cooling_preset = c.preset;
        if (s.cooling_policy)
          s.cooling_policy = {
            ...s.cooling_policy,
            requested_preset: c.preset,
            effective_preset: c.preset,
          };
        this.emit();
        return ok();
      }
      case 'set_select': {
        if (c.entity_id === 'audio_output' && s.session?.audio) {
          s.session = { ...s.session, audio: { ...s.session.audio, output_id: c.option } };
          this.emit();
          return ok();
        }
        return err('not_supported');
      }
      case 'set_number': {
        const a = needsAgent();
        if (a) return a;
        if (s.session?.audio)
          s.session = { ...s.session, audio: { ...s.session.audio, volume: c.value } };
        this.emit();
        return ok();
      }
      case 'set_switch': {
        const a = needsAgent();
        if (a) return a;
        if (s.session?.audio)
          s.session = { ...s.session, audio: { ...s.session.audio, muted: c.state } };
        this.emit();
        return ok();
      }
      case 'press_button': {
        if (c.entity_id === 'power_lock' || c.entity_id === 'display_off')
          return needsAgent() ?? ok();
        if (c.entity_id.startsWith('script_')) {
          if (!s.permissions.custom_scripts) return err('permission_denied');
          return needsAgent() ?? ok('backup finished: 128 files copied');
        }
        return ok(`${c.entity_id} scheduled`);
      }
      case 'check_os_updates': {
        s.updates = { ...s.updates, in_progress: true };
        this.emit();
        setTimeout(() => {
          s.updates = { ...s.updates, in_progress: false, last_checked: now() };
          this.emit();
        }, 1800);
        return ok();
      }
      case 'install_os_updates': {
        s.updates = { ...s.updates, in_progress: true };
        this.emit();
        setTimeout(() => {
          s.updates = {
            ...s.updates,
            in_progress: false,
            pending_count: 0,
            pending: [],
            titles: [],
            kb_ids: [],
            reboot_required: true,
          };
          this.emit();
        }, 2500);
        return ok();
      }
      case 'send_notification':
        return needsAgent() ?? ok();
    }
  }
}
