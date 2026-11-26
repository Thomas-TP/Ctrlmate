import type { AppInfo, ConnectionStatus } from '../api/app';
import type { DaemonSnapshot } from '../types/protocol';

/**
 * Support bundle for "copy diagnostics". Contains versions, capabilities and states only.
 * It deliberately never includes the IPC token, pairing PIN, MQTT host / user / password, MAC
 * address, host name, device id, paired client names, script paths or window titles.
 */
export function buildDiagnostics(input: {
  app: AppInfo | null;
  connection: ConnectionStatus;
  snapshot: DaemonSnapshot | null;
  locale: string;
  now?: Date;
}): string {
  const { app, connection, snapshot: s, locale } = input;
  const bundle = {
    generated_at: (input.now ?? new Date()).toISOString(),
    app: app && {
      version: app.version,
      platform: app.platform,
      arch: app.arch,
      debug_build: app.debug,
      ipc_port: app.ipc_port,
      token_source: app.token_source,
      update_channel: app.update_channel,
    },
    ui_locale: locale,
    connection: {
      status: connection.status,
      reason: connection.reason ?? null,
      attempt: connection.attempt,
    },
    daemon: s && {
      service: s.service,
      protocol_version: s.manifest.protocol_version,
      security_mode: s.manifest.host.security_mode,
      deep_bridge_active: s.deep_bridge_active,
      polling_profile: s.polling_profile,
      temperature_unit: s.temperature_unit,
      permissions: s.permissions,
      counts: {
        entities: s.manifest.entities.length,
        states: s.states.length,
        child_devices: s.manifest.child_devices.length,
        hid_devices: s.hid_devices.length,
        paired_clients: s.paired_clients.length,
        scripts: s.scripts.length,
        conflicts: s.conflicts.length,
      },
      sensor_status: s.sensor_status,
      rgb_backends: s.rgb_backends,
      rgb_devices: s.rgb_devices.map((d) => ({
        id: d.id,
        zone_id: d.zone_id,
        transport: d.transport,
        controllable: d.controllable,
        reason: d.reason,
      })),
      conflicts_running: s.conflicts.filter((c) => c.running).map((c) => c.id),
      cooling: s.cooling_policy && {
        requested: s.cooling_policy.requested_preset,
        effective: s.cooling_policy.effective_preset,
        state: s.cooling_policy.state,
        reason: s.cooling_policy.reason ?? null,
        thresholds: s.cooling_policy.thresholds,
      },
      mqtt: {
        enabled: s.mqtt.enabled,
        port: s.mqtt.port,
        use_tls: s.mqtt.use_tls,
        connected: s.mqtt_connected,
        password_set: s.mqtt_password_set,
      },
      updates: {
        checked: s.updates.checked,
        pending_count: s.updates.pending_count,
        reboot_required: s.updates.reboot_required,
        in_progress: s.updates.in_progress,
        error: s.updates.error ?? null,
      },
    },
  };
  return JSON.stringify(bundle, null, 2);
}
