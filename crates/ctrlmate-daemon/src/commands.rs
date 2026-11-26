//! Command execution (Home Assistant, MQTT, desktop UI) and settings changes.
//!
//! Permission model: the `RemotePermissions` toggles gate what *remote* callers (Home Assistant
//! over LAN, MQTT) may do. `custom_scripts` additionally gates execution for everyone. Results are
//! `CommandResult` with stable codes; no human sentences are produced here.

use crate::scripts::validate_scripts;
use crate::sessions::SessionBackend;
use crate::state::AppState;
use chrono::Utc;
use ctrlmate_hal::os_control::execute_power_action;
use ctrlmate_protocol::{
    CommandResult, ControlCommand, CoolingPreset, EntityState, EntityValue, ResultCode,
    SessionAction, SettingsUpdate,
};
use std::sync::Arc;
use tracing::error;

fn denied() -> CommandResult {
    CommandResult::err(ResultCode::PermissionDenied)
}

fn invalid(detail: &str) -> CommandResult {
    CommandResult::err_with(ResultCode::InvalidArgument, detail)
}

fn text_state(id: &str, v: String) -> EntityState {
    EntityState {
        id: id.into(),
        value: EntityValue::Text(v),
        attributes: Default::default(),
        updated_at: Utc::now(),
    }
}

impl AppState {
    pub async fn execute_command(
        self: &Arc<Self>,
        cmd: ControlCommand,
        from_remote: bool,
    ) -> CommandResult {
        let perms = self.config.read().await.permissions.clone();

        match cmd {
            ControlCommand::SetLight {
                entity_id,
                is_on,
                brightness,
                rgb,
                color_temp_kelvin,
                transition_ms,
            } => {
                if from_remote && !perms.lighting {
                    return denied();
                }
                if let Some(k) = color_temp_kelvin
                    && !(ctrlmate_protocol::MIN_COLOR_TEMP_KELVIN
                        ..=ctrlmate_protocol::MAX_COLOR_TEMP_KELVIN)
                        .contains(&k)
                {
                    return invalid("color_temp_kelvin out of range");
                }
                let Some(updated) = self
                    .hw
                    .apply_light(
                        entity_id,
                        is_on,
                        brightness,
                        rgb,
                        color_temp_kelvin,
                        transition_ms,
                    )
                    .await
                else {
                    return CommandResult::err(ResultCode::Internal);
                };
                if updated.is_empty() {
                    return invalid("unknown lighting entity");
                }
                let now = Utc::now();
                let batch = updated
                    .into_iter()
                    .map(|(id, st)| EntityState {
                        id,
                        value: EntityValue::Light(st),
                        attributes: Default::default(),
                        updated_at: now,
                    })
                    .collect();
                self.merge_and_broadcast_states(batch).await;
                CommandResult::ok()
            }
            ControlCommand::SetCoolingPreset { preset, .. } => {
                if from_remote && !perms.cooling {
                    return denied();
                }
                self.set_requested_cooling(preset).await
            }
            ControlCommand::SetSelect { entity_id, option } => match entity_id.as_str() {
                "cooling_preset" => {
                    if from_remote && !perms.cooling {
                        return denied();
                    }
                    match CoolingPreset::from_ha_option(&option) {
                        Some(p) => self.set_requested_cooling(p).await,
                        None => invalid("unknown cooling preset"),
                    }
                }
                "audio_output" => {
                    if from_remote && !perms.audio_and_media {
                        return denied();
                    }
                    // HA shows output names; the agent wants ids.
                    let id = self
                        .sessions
                        .state()
                        .and_then(|s| s.audio)
                        .and_then(|a| {
                            a.outputs
                                .into_iter()
                                .find(|o| o.name == option || o.id == option)
                        })
                        .map(|o| o.id);
                    match id {
                        Some(id) => {
                            self.session_action(SessionAction::SetAudioOutput { id })
                                .await
                        }
                        None => invalid("unknown audio output"),
                    }
                }
                _ => CommandResult::err(ResultCode::NotSupported),
            },
            ControlCommand::SetNumber { entity_id, value } => {
                if entity_id != "master_volume" {
                    return CommandResult::err(ResultCode::NotSupported);
                }
                if from_remote && !perms.audio_and_media {
                    return denied();
                }
                if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                    return invalid("volume must be within 0..=100");
                }
                self.session_action(SessionAction::SetVolume { percent: value })
                    .await
            }
            ControlCommand::SetSwitch { entity_id, state } => {
                if entity_id != "audio_mute" {
                    return CommandResult::err(ResultCode::NotSupported);
                }
                if from_remote && !perms.audio_and_media {
                    return denied();
                }
                self.session_action(SessionAction::SetMute { muted: state })
                    .await
            }
            ControlCommand::PressButton { entity_id } => {
                if entity_id == "check_os_updates" {
                    if from_remote && !perms.os_updates {
                        return denied();
                    }
                    return self.spawn_update_scan();
                }
                if entity_id.starts_with("power_") || entity_id == "display_off" {
                    if from_remote && !perms.power_control {
                        return denied();
                    }
                    return self.power_action(&entity_id).await;
                }
                if let Some(script_id) = entity_id.strip_prefix("script_") {
                    // Gates execution for everyone, local UI included.
                    if !perms.custom_scripts {
                        return denied();
                    }
                    let known = self
                        .config
                        .read()
                        .await
                        .scripts
                        .iter()
                        .any(|s| s.id == script_id && s.enabled);
                    if !known {
                        return invalid("unknown or disabled script");
                    }
                    return self
                        .session_action(SessionAction::RunScript {
                            script_id: script_id.to_string(),
                        })
                        .await;
                }
                CommandResult::err(ResultCode::NotSupported)
            }
            ControlCommand::CheckOsUpdates => {
                if from_remote && !perms.os_updates {
                    return denied();
                }
                self.spawn_update_scan()
            }
            ControlCommand::InstallOsUpdates => {
                if from_remote && !perms.os_updates {
                    return denied();
                }
                self.spawn_update_install()
            }
            ControlCommand::SendNotification { title, message } => {
                if from_remote && !perms.notifications {
                    return denied();
                }
                if title.chars().count() > 128 || message.chars().count() > 1024 {
                    return invalid("notification too long");
                }
                self.session_action(SessionAction::Notify {
                    title,
                    body: message,
                })
                .await
            }
        }
    }

    async fn session_action(&self, action: SessionAction) -> CommandResult {
        self.sessions.execute(action).await
    }

    /// `power_lock` and `display_off` belong to the user session; the rest are machine-wide.
    async fn power_action(&self, id: &str) -> CommandResult {
        match id {
            "power_lock" => self.session_action(SessionAction::Lock).await,
            "display_off" => self.session_action(SessionAction::DisplayOff).await,
            "power_shutdown" | "power_restart" | "power_sleep" | "power_hibernate" => {
                let id = id.to_string();
                match tokio::task::spawn_blocking(move || execute_power_action(&id)).await {
                    Ok(Ok(msg)) => CommandResult::ok_with(msg),
                    Ok(Err(e)) => CommandResult::err_with(ResultCode::HardwareError, e.to_string()),
                    Err(e) => CommandResult::err_with(ResultCode::Internal, e.to_string()),
                }
            }
            _ => CommandResult::err(ResultCode::NotSupported),
        }
    }

    async fn set_requested_cooling(self: &Arc<Self>, preset: CoolingPreset) -> CommandResult {
        {
            let mut cfg = self.config.write().await;
            cfg.cooling_preset = preset;
            if let Err(e) = cfg.save(&self.data_dir) {
                error!("Failed persisting cooling preset: {e:#}");
            }
        }
        self.apply_cooling(true).await;
        self.merge_and_broadcast_states(vec![text_state(
            "cooling_preset",
            preset.as_ha_option().to_string(),
        )])
        .await;
        CommandResult::ok()
    }

    /// Applies a validated settings change (IPC only).
    pub async fn apply_settings(self: &Arc<Self>, req: SettingsUpdate) -> CommandResult {
        if let Some(scripts) = &req.scripts
            && let Err(detail) = validate_scripts(scripts)
        {
            return invalid(&detail);
        }

        let mut manifest_dirty = false;
        let mut telemetry_turned_on = false;
        let mut wake = false;
        let mut mqtt_update = None;
        let mut security_mode = None;
        let mut thermal = None;
        let pushed_scripts;
        {
            let mut cfg = self.config.write().await;
            if let Some(p) = req.permissions {
                telemetry_turned_on = p.telemetry && !cfg.permissions.telemetry;
                self.telemetry
                    .store(p.telemetry, std::sync::atomic::Ordering::Relaxed);
                cfg.permissions = p;
            }
            if let Some(profile) = req.polling_profile {
                cfg.polling_profile = profile;
                crate::state::lock(&self.poller).set_profile(profile);
                wake = true;
            }
            if let Some(unit) = req.temperature_unit {
                cfg.temperature_unit = unit;
            }
            if let Some(mode) = req.security_mode
                && mode != cfg.security_mode
            {
                cfg.security_mode = mode;
                security_mode = Some(mode);
                manifest_dirty = true;
            }
            if let Some(scripts) = req.scripts {
                cfg.scripts = scripts;
                manifest_dirty = true;
            }
            if let Some(t) = req.thermal {
                let t = ctrlmate_hal::sanitize_thresholds(t);
                cfg.thermal = t;
                thermal = Some(t);
            }
            if let Some(v) = req.include_optional_updates {
                cfg.include_optional_updates = v;
            }
            if let Some(mut mqtt) = req.mqtt {
                // Absent password keeps the stored one; an empty string clears it.
                match mqtt.password.take() {
                    None => mqtt.password = cfg.mqtt.password.clone(),
                    Some(p) if p.is_empty() => mqtt.password = None,
                    Some(p) => mqtt.password = Some(p),
                }
                cfg.mqtt = mqtt.clone();
                mqtt_update = Some(mqtt);
            }
            pushed_scripts = cfg.scripts.clone();
            if let Err(e) = cfg.save(&self.data_dir) {
                error!("Failed persisting settings: {e:#}");
                return CommandResult::err_with(
                    ResultCode::Internal,
                    "settings could not be saved",
                );
            }
        }

        if let Some(mode) = security_mode {
            // Blocking driver start / stop happens on the hardware thread.
            let _ = self.hw.set_security_mode(mode).await;
        }
        if let Some(t) = thermal {
            // Takes effect on the cooling thread's next tick; no restart needed.
            self.hw.set_thermal_thresholds(t);
        }
        if let Some(m) = mqtt_update {
            let _ = self.mqtt_cfg_tx.send(m);
        }
        self.sessions.agent.push_scripts(&pushed_scripts);
        if manifest_dirty {
            self.refresh_manifest().await;
        }
        if telemetry_turned_on {
            // Consumers were muted: resend everything, not just future deltas.
            crate::state::lock(&self.poller).reset();
            let (_, batch) = self.resync_frames().await;
            let _ = self.ha_tx.send(batch);
        }
        if wake {
            self.sampler_wake.notify_one();
        }
        self.notify_ui().await;
        CommandResult::ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testutil::{rig, rig_with};
    use ctrlmate_protocol::{
        AllowlistedScript, AudioOutput, AudioState, DaemonToAgent, MqttConfig, RemotePermissions,
        RgbColor, SessionState,
    };
    use std::time::Duration;

    fn press(id: &str) -> ControlCommand {
        ControlCommand::PressButton {
            entity_id: id.into(),
        }
    }

    async fn set_perms(st: &Arc<AppState>, f: impl FnOnce(&mut RemotePermissions)) {
        let mut p = st.config.read().await.permissions.clone();
        f(&mut p);
        let r = st
            .apply_settings(SettingsUpdate {
                permissions: Some(p),
                ..SettingsUpdate::default()
            })
            .await;
        assert!(r.ok);
    }

    fn script(id: &str) -> AllowlistedScript {
        AllowlistedScript {
            id: id.into(),
            name: id.into(),
            executable: if cfg!(windows) {
                r"C:\tools\x.exe".into()
            } else {
                "/bin/x".into()
            },
            args: vec![],
            working_dir: None,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn session_features_without_agent_return_no_agent() {
        let rig = rig().await;
        let st = &rig.state;
        for cmd in [
            press("power_lock"),
            press("display_off"),
            ControlCommand::SetNumber {
                entity_id: "master_volume".into(),
                value: 10.0,
            },
            ControlCommand::SetSwitch {
                entity_id: "audio_mute".into(),
                state: true,
            },
            ControlCommand::SendNotification {
                title: "t".into(),
                message: "m".into(),
            },
        ] {
            let r = st.execute_command(cmd.clone(), false).await;
            assert!(!r.ok, "{cmd:?}");
            assert_eq!(r.code, "no_agent", "{cmd:?}");
        }
    }

    #[tokio::test]
    async fn permissions_gate_remote_but_not_local() {
        let rig = rig().await;
        let st = &rig.state;
        set_perms(st, |p| {
            p.lighting = false;
            p.cooling = false;
            p.audio_and_media = false;
            p.power_control = false;
            p.notifications = false;
            p.os_updates = false;
        })
        .await;
        let light = ControlCommand::SetLight {
            entity_id: "light_global".into(),
            is_on: Some(false),
            brightness: None,
            rgb: None,
            color_temp_kelvin: None,
            transition_ms: None,
        };
        assert_eq!(
            st.execute_command(light.clone(), true).await.code,
            "permission_denied"
        );
        assert!(st.execute_command(light, false).await.ok);
        for cmd in [
            ControlCommand::SetSelect {
                entity_id: "cooling_preset".into(),
                option: "silent".into(),
            },
            ControlCommand::SetNumber {
                entity_id: "master_volume".into(),
                value: 1.0,
            },
            press("power_sleep"),
            ControlCommand::CheckOsUpdates,
            ControlCommand::SendNotification {
                title: "a".into(),
                message: "b".into(),
            },
        ] {
            assert_eq!(
                st.execute_command(cmd.clone(), true).await.code,
                "permission_denied",
                "{cmd:?}"
            );
        }
        // Local UI is not gated by the HA toggles.
        assert!(
            st.execute_command(
                ControlCommand::SetSelect {
                    entity_id: "cooling_preset".into(),
                    option: "silent".into()
                },
                false
            )
            .await
            .ok
        );
    }

    #[tokio::test]
    async fn thermal_thresholds_update_at_runtime_are_clamped_persisted_and_pushed() {
        let rig = rig().await;
        let st = &rig.state;
        assert_eq!(
            st.snapshot().await.cooling_policy.thresholds.cpu_critical_c,
            93.0
        );
        let r = st
            .apply_settings(SettingsUpdate {
                thermal: Some(ctrlmate_protocol::ThermalThresholds {
                    cpu_critical_c: 120.0,
                    ..ctrlmate_protocol::ThermalThresholds::default()
                }),
                include_optional_updates: Some(true),
                ..SettingsUpdate::default()
            })
            .await;
        assert!(r.ok);
        let snap = st.snapshot().await;
        assert_eq!(
            snap.cooling_policy.thresholds.cpu_critical_c, 98.0,
            "clamped to 85-98"
        );
        assert!(snap.include_optional_updates);
        // The hardware thread received the (clamped) value.
        let mut pushed = false;
        for _ in 0..50 {
            if rig
                .log
                .lock()
                .unwrap()
                .thresholds
                .is_some_and(|t| t.cpu_critical_c == 98.0)
            {
                pushed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(pushed, "thresholds must reach the cooling engine");
        // Persisted.
        let cfg = crate::config::DaemonConfig::load_or_create(&st.data_dir).unwrap();
        assert_eq!(cfg.thermal.cpu_critical_c, 98.0);
    }

    #[tokio::test]
    async fn custom_scripts_gate_everyone_and_only_run_via_agent() {
        let rig = rig().await;
        let st = &rig.state;
        let r = st
            .apply_settings(SettingsUpdate {
                scripts: Some(vec![script("backup")]),
                ..SettingsUpdate::default()
            })
            .await;
        assert!(r.ok);
        // Toggle off by default, even for the local UI.
        assert_eq!(
            st.execute_command(press("script_backup"), false).await.code,
            "permission_denied"
        );
        assert_eq!(
            st.execute_command(press("script_backup"), true).await.code,
            "permission_denied"
        );
        set_perms(st, |p| p.custom_scripts = true).await;
        // Unknown script.
        assert_eq!(
            st.execute_command(press("script_nope"), false).await.code,
            "invalid_argument"
        );
        // Known script but no agent: never spawned by the daemon.
        assert_eq!(
            st.execute_command(press("script_backup"), false).await.code,
            "no_agent"
        );
        // With an agent the request reaches it with the script id only.
        let mut att = st.sessions.agent.attach();
        let st2 = Arc::clone(st);
        let conn = att.conn_id;
        tokio::spawn(async move {
            if let Some(DaemonToAgent::Request { id, action }) = att.outgoing.recv().await {
                assert_eq!(
                    action,
                    SessionAction::RunScript {
                        script_id: "backup".into()
                    }
                );
                st2.sessions
                    .agent
                    .resolve(conn, &id, CommandResult::ok_with("done"));
            }
        });
        // `Scripts` frame was queued first; the request follows. Drain via the task above.
        let r = st.execute_command(press("script_backup"), false).await;
        // The first outgoing frame may be the Scripts push (sent before attach: not queued), so
        // the request is the first frame.
        assert!(r.ok, "{r:?}");
        assert_eq!(r.message.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn disabled_scripts_cannot_run() {
        let rig = rig().await;
        let st = &rig.state;
        let mut s = script("off");
        s.enabled = false;
        st.apply_settings(SettingsUpdate {
            scripts: Some(vec![s]),
            ..SettingsUpdate::default()
        })
        .await;
        set_perms(st, |p| p.custom_scripts = true).await;
        assert_eq!(
            st.execute_command(press("script_off"), false).await.code,
            "invalid_argument"
        );
    }

    #[tokio::test]
    async fn script_allowlist_edit_is_validated() {
        let rig = rig().await;
        let mut bad = script("Bad Id");
        bad.executable = "relative.exe".into();
        let r = rig
            .state
            .apply_settings(SettingsUpdate {
                scripts: Some(vec![bad]),
                ..SettingsUpdate::default()
            })
            .await;
        assert_eq!(r.code, "invalid_argument");
        assert!(rig.state.config.read().await.scripts.is_empty());
        // Valid edit refreshes the manifest with a button entity.
        let ok = rig
            .state
            .apply_settings(SettingsUpdate {
                scripts: Some(vec![script("good")]),
                ..SettingsUpdate::default()
            })
            .await;
        assert!(ok.ok);
        let m = rig.state.manifest.read().await;
        assert!(m.entities.iter().any(|e| e.id == "script_good"));
    }

    #[tokio::test]
    async fn volume_is_validated_and_forwarded_to_the_agent() {
        let rig = rig_with(false, Duration::from_millis(300)).await;
        let st = &rig.state;
        let bad = ControlCommand::SetNumber {
            entity_id: "master_volume".into(),
            value: 150.0,
        };
        assert_eq!(
            st.execute_command(bad, false).await.code,
            "invalid_argument"
        );
        let nan = ControlCommand::SetNumber {
            entity_id: "master_volume".into(),
            value: f64::NAN,
        };
        assert_eq!(
            st.execute_command(nan, false).await.code,
            "invalid_argument"
        );

        let mut att = st.sessions.agent.attach();
        st.sessions.agent.set_state(
            att.conn_id,
            SessionState {
                audio: Some(AudioState {
                    volume: 10.0,
                    muted: false,
                    output_id: Some("a".into()),
                    outputs: vec![
                        AudioOutput {
                            id: "a".into(),
                            name: "Speakers".into(),
                        },
                        AudioOutput {
                            id: "b".into(),
                            name: "Headset".into(),
                        },
                    ],
                }),
                ..SessionState::default()
            },
        );
        let st2 = Arc::clone(st);
        let conn = att.conn_id;
        let seen = Arc::new(std::sync::Mutex::new(vec![]));
        let seen2 = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Some(DaemonToAgent::Request { id, action }) = att.outgoing.recv().await {
                seen2.lock().unwrap().push(action);
                st2.sessions.agent.resolve(conn, &id, CommandResult::ok());
            }
        });
        assert!(
            st.execute_command(
                ControlCommand::SetNumber {
                    entity_id: "master_volume".into(),
                    value: 33.0
                },
                true
            )
            .await
            .ok
        );
        // HA sends the *name*; the agent receives the id.
        assert!(
            st.execute_command(
                ControlCommand::SetSelect {
                    entity_id: "audio_output".into(),
                    option: "Headset".into()
                },
                true
            )
            .await
            .ok
        );
        assert_eq!(
            st.execute_command(
                ControlCommand::SetSelect {
                    entity_id: "audio_output".into(),
                    option: "Nope".into()
                },
                true
            )
            .await
            .code,
            "invalid_argument"
        );
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0], SessionAction::SetVolume { percent: 33.0 });
        assert_eq!(seen[1], SessionAction::SetAudioOutput { id: "b".into() });
    }

    #[tokio::test]
    async fn cooling_preset_is_persisted_applied_and_gaming_is_selectable() {
        let rig = rig().await;
        let st = &rig.state;
        let r = st
            .execute_command(
                ControlCommand::SetSelect {
                    entity_id: "cooling_preset".into(),
                    option: "gaming".into(),
                },
                true,
            )
            .await;
        assert!(r.ok);
        assert_eq!(st.config.read().await.cooling_preset, CoolingPreset::Gaming);
        let on_disk = crate::config::DaemonConfig::load_or_create(&st.data_dir).unwrap();
        assert_eq!(on_disk.cooling_preset, CoolingPreset::Gaming);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            rig.log
                .lock()
                .unwrap()
                .calls
                .contains(&"preset:gaming".to_string())
        );
        let bad = st
            .execute_command(
                ControlCommand::SetSelect {
                    entity_id: "cooling_preset".into(),
                    option: "full_speed".into(),
                },
                true,
            )
            .await;
        assert_eq!(bad.code, "invalid_argument");
    }

    #[tokio::test]
    async fn light_validation_and_unknown_entities() {
        let rig = rig().await;
        let st = &rig.state;
        let mk = |entity: &str, k: Option<u16>| ControlCommand::SetLight {
            entity_id: entity.into(),
            is_on: Some(true),
            brightness: Some(100),
            rgb: Some(RgbColor { r: 1, g: 2, b: 3 }),
            color_temp_kelvin: k,
            transition_ms: None,
        };
        assert_eq!(
            st.execute_command(mk("light_nope", None), true).await.code,
            "invalid_argument"
        );
        assert_eq!(
            st.execute_command(mk("light_global", Some(100)), true)
                .await
                .code,
            "invalid_argument"
        );
        assert!(
            st.execute_command(mk("light_global", Some(3000)), true)
                .await
                .ok
        );
    }

    #[tokio::test]
    async fn busy_while_an_update_operation_runs() {
        let rig = rig().await;
        let st = &rig.state;
        st.update_busy
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            st.execute_command(ControlCommand::CheckOsUpdates, false)
                .await
                .code,
            "busy"
        );
        assert_eq!(
            st.execute_command(ControlCommand::InstallOsUpdates, false)
                .await
                .code,
            "busy"
        );
    }

    #[tokio::test]
    async fn mqtt_password_is_kept_cleared_or_replaced() {
        let rig = rig().await;
        let st = &rig.state;
        let cfg = |pw: Option<&str>| SettingsUpdate {
            mqtt: Some(MqttConfig {
                enabled: false,
                host: "broker".into(),
                password: pw.map(str::to_string),
                ..MqttConfig::default()
            }),
            ..SettingsUpdate::default()
        };
        st.apply_settings(cfg(Some("secret"))).await;
        assert_eq!(
            st.config.read().await.mqtt.password.as_deref(),
            Some("secret")
        );
        st.apply_settings(cfg(None)).await;
        assert_eq!(
            st.config.read().await.mqtt.password.as_deref(),
            Some("secret")
        );
        let snap = st.snapshot().await;
        assert!(snap.mqtt_password_set);
        assert!(snap.mqtt.password.is_none());
        st.apply_settings(cfg(Some(""))).await;
        assert_eq!(st.config.read().await.mqtt.password, None);
    }

    #[tokio::test]
    async fn telemetry_permission_toggle_updates_flag_and_resyncs() {
        let rig = rig().await;
        let st = &rig.state;
        let mut ha = st.ha_tx.subscribe();
        set_perms(st, |p| p.telemetry = false).await;
        assert!(!st.telemetry_allowed());
        while ha.try_recv().is_ok() {}
        set_perms(st, |p| p.telemetry = true).await;
        assert!(st.telemetry_allowed());
        let mut saw_full = false;
        while let Ok(m) = ha.try_recv() {
            if let ctrlmate_protocol::WireMessage::StateBatch { states } = m
                && states.len() > 3
            {
                saw_full = true;
            }
        }
        assert!(saw_full, "re-enabling telemetry must resend all states");
    }
}
