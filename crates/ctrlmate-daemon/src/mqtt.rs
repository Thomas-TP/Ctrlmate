//! Secondary Home Assistant 2026.11 MQTT Device + Child Device Discovery transport.
//!
//! Uses retained Device Discovery payloads (`<prefix>/device/<id>/config`) with native
//! JSON schema `light` entities (`rgb` + `color_temp` 2000K–6500K), canonical °C sensors,
//! `via_device` Child Devices, unified LWT availability, and strict Ctrlmate permission checks.

use crate::server::AppState;
use ctrlmate_protocol::{
    ControlCommand, DeviceManifest, EntityDescriptor, EntityPlatform, EntityState, EntityValue,
    MqttConfig, RgbColor, WireMessage, MAX_COLOR_TEMP_KELVIN, MIN_COLOR_TEMP_KELVIN,
};
use rumqttc::{AsyncClient, Event, Incoming, LastWill, MqttOptions, QoS, Transport};
use serde_json::{json, Map, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Spawns the live-reloadable MQTT worker bound to `mqtt_cfg_rx`.
pub fn spawn_mqtt_worker(state: Arc<AppState>, mut mqtt_cfg_rx: watch::Receiver<MqttConfig>) {
    tokio::spawn(async move {
        loop {
            let cfg = mqtt_cfg_rx.borrow_and_update().clone();
            if !cfg.enabled || cfg.host.trim().is_empty() {
                set_connected(&state, false).await;
                if mqtt_cfg_rx.changed().await.is_err() {
                    break;
                }
                continue;
            }

            info!(
                "Starting Home Assistant 2026.11 MQTT Discovery client ({}:{}, tls={})",
                cfg.host, cfg.port, cfg.use_tls
            );

            tokio::select! {
                _ = run_mqtt_session(Arc::clone(&state), cfg) => {
                    set_connected(&state, false).await;
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                res = mqtt_cfg_rx.changed() => {
                    set_connected(&state, false).await;
                    if res.is_err() {
                        break;
                    }
                    info!("MQTT configuration changed — reloading MQTT transport");
                }
            }
        }
    });
}

async fn set_connected(state: &Arc<AppState>, connected: bool) {
    let prev = state.mqtt_connected.swap(connected, Ordering::Relaxed);
    if prev != connected {
        let _ = state.ui_tx.send(state.snapshot().await);
    }
}

async fn run_mqtt_session(state: Arc<AppState>, cfg: MqttConfig) {
    let manifest = state.latest_manifest.read().await.clone();
    let dev_id = sanitize_id(&manifest.host.device_id);
    let base_topic = format!("ctrlmate/{dev_id}");
    let avail_topic = format!("{base_topic}/status");
    let client_id = format!("ctrlmate_{}", &dev_id[..dev_id.len().min(16)]);

    let mut opts = MqttOptions::new(client_id, cfg.host.trim(), cfg.port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);
    opts.set_last_will(LastWill::new(&avail_topic, "offline", QoS::AtLeastOnce, true));

    if let Some(user) = cfg.username.as_deref().filter(|s| !s.is_empty()) {
        opts.set_credentials(user, cfg.password.as_deref().unwrap_or_default());
    }
    if cfg.use_tls {
        opts.set_transport(Transport::tls_with_default_config());
    }

    let (client, mut eventloop) = AsyncClient::new(opts, 128);
    let mut ha_rx = state.ha_tx.subscribe();

    loop {
        tokio::select! {
            ev = eventloop.poll() => {
                match ev {
                    Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                        info!("MQTT connected to {}:{}", cfg.host, cfg.port);
                        set_connected(&state, true).await;

                        let _ = client
                            .publish(&avail_topic, QoS::AtLeastOnce, true, "online")
                            .await;
                        let _ = client
                            .subscribe(format!("{base_topic}/cmd/#"), QoS::AtLeastOnce)
                            .await;

                        let cur_manifest = state.latest_manifest.read().await.clone();
                        publish_discovery(&client, &cfg.discovery_prefix, &base_topic, &cur_manifest).await;

                        let cur_states = state.latest_states.read().await.clone();
                        publish_states(&client, &base_topic, &cur_states).await;
                    }
                    Ok(Event::Incoming(Incoming::Publish(p))) => {
                        let prefix = format!("{base_topic}/cmd/");
                        if let Some(entity_id) = p.topic.strip_prefix(&prefix) {
                            let payload = String::from_utf8_lossy(&p.payload).to_string();
                            handle_mqtt_command(&state, entity_id, &payload).await;
                        }
                    }
                    Ok(_) => {}
                    Err(err) => {
                        warn!("MQTT connection error: {err}");
                        break;
                    }
                }
            }
            wire = ha_rx.recv() => {
                match wire {
                    Ok(WireMessage::Manifest(new_manifest)) => {
                        publish_discovery(&client, &cfg.discovery_prefix, &base_topic, &new_manifest).await;
                    }
                    Ok(WireMessage::StateBatch { states }) => {
                        if state.mqtt_connected.load(Ordering::Relaxed) {
                            publish_states(&client, &base_topic, &states).await;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        }
    }
}

async fn publish_discovery(
    client: &AsyncClient,
    discovery_prefix: &str,
    base_topic: &str,
    manifest: &DeviceManifest,
) {
    let prefix = discovery_prefix.trim_end_matches('/');
    let host_ident = format!("ctrlmate_{}", sanitize_id(&manifest.host.device_id));
    let avail_topic = format!("{base_topic}/status");

    let origin = json!({
        "name": "Ctrlmate",
        "sw": manifest.host.sw_version,
        "url": "https://github.com/Thomas-TP/Ctrlmate",
    });

    // 1. Parent PC Device Discovery payload
    let mut parent_cmps = Map::new();
    for ent in manifest.entities.iter().filter(|e| e.exposed && e.child_device_id.is_none()) {
        if let Some(cmp) = build_component_payload(&host_ident, base_topic, ent) {
            parent_cmps.insert(ent.id.clone(), cmp);
        }
    }
    let mut host_dev = json!({
        "ids": [host_ident],
        "name": manifest.host.name,
        "mf": manifest.host.manufacturer,
        "mdl": manifest.host.model,
        "sw": manifest.host.sw_version,
    });
    if let Some(mac) = &manifest.host.mac_address {
        host_dev["cns"] = json!([["mac", mac]]);
    }

    let parent_payload = json!({
        "dev": host_dev,
        "o": origin,
        "avty_t": avail_topic,
        "pl_avail": "online",
        "pl_not_avail": "offline",
        "cmps": parent_cmps,
    });
    let parent_topic = format!("{prefix}/device/{host_ident}/config");
    if let Ok(body) = serde_json::to_string(&parent_payload) {
        let _ = client.publish(parent_topic, QoS::AtLeastOnce, true, body).await;
    }

    // 2. Child Devices Discovery payloads (linked via `via_device` to parent PC)
    for child in &manifest.child_devices {
        let child_ident = format!("{host_ident}_{}", sanitize_id(&child.id));
        let mut child_cmps = Map::new();
        for ent in manifest
            .entities
            .iter()
            .filter(|e| e.exposed && e.child_device_id.as_deref() == Some(child.id.as_str()))
        {
            if let Some(cmp) = build_component_payload(&host_ident, base_topic, ent) {
                child_cmps.insert(ent.id.clone(), cmp);
            }
        }
        if child_cmps.is_empty() {
            continue;
        }
        let child_payload = json!({
            "dev": {
                "ids": [child_ident],
                "name": child.name,
                "mf": child.manufacturer.as_deref().unwrap_or("Ctrlmate"),
                "mdl": child.model.as_deref().unwrap_or(&child.name),
                "via_device": host_ident,
            },
            "o": origin,
            "avty_t": avail_topic,
            "pl_avail": "online",
            "pl_not_avail": "offline",
            "cmps": child_cmps,
        });
        let child_topic = format!("{prefix}/device/{child_ident}/config");
        if let Ok(body) = serde_json::to_string(&child_payload) {
            let _ = client.publish(child_topic, QoS::AtLeastOnce, true, body).await;
        }
    }

    debug!(
        "Published MQTT Device Discovery for '{}' ({} child devices)",
        manifest.host.name,
        manifest.child_devices.len()
    );
}

fn build_component_payload(
    host_ident: &str,
    base_topic: &str,
    ent: &EntityDescriptor,
) -> Option<Value> {
    let unique_id = format!("{host_ident}_{}", sanitize_id(&ent.id));
    let state_topic = format!("{base_topic}/state/{}", ent.id);
    let cmd_topic = format!("{base_topic}/cmd/{}", ent.id);

    let mut obj = Map::new();
    obj.insert("name".into(), json!(ent.name));
    obj.insert("uniq_id".into(), json!(unique_id));

    if let Some(icon) = &ent.icon {
        obj.insert("ic".into(), json!(icon));
    }
    if let Some(dc) = &ent.device_class {
        obj.insert("dev_cla".into(), json!(dc));
    }
    if let Some(cat) = ent.entity_category {
        let s = match cat {
            ctrlmate_protocol::EntityCategory::Config => "config",
            ctrlmate_protocol::EntityCategory::Diagnostic => "diagnostic",
        };
        obj.insert("ent_cat".into(), json!(s));
    }

    match ent.platform {
        EntityPlatform::Sensor => {
            obj.insert("p".into(), json!("sensor"));
            obj.insert("stat_t".into(), json!(state_topic));
            if let Some(unit) = &ent.unit_of_measurement {
                obj.insert("unit_of_meas".into(), json!(unit));
            }
            if let Some(sc) = ent.state_class {
                let s = match sc {
                    ctrlmate_protocol::StateClass::Measurement => "measurement",
                    ctrlmate_protocol::StateClass::Total => "total",
                    ctrlmate_protocol::StateClass::TotalIncreasing => "total_increasing",
                };
                obj.insert("stat_cla".into(), json!(s));
            }
            if let Some(prec) = ent.suggested_display_precision {
                obj.insert("sug_dsp_prc".into(), json!(prec));
            }
        }
        EntityPlatform::BinarySensor => {
            obj.insert("p".into(), json!("binary_sensor"));
            obj.insert("stat_t".into(), json!(state_topic));
            obj.insert("pl_on".into(), json!("ON"));
            obj.insert("pl_off".into(), json!("OFF"));
        }
        EntityPlatform::Light => {
            obj.insert("p".into(), json!("light"));
            obj.insert("schema".into(), json!("json"));
            obj.insert("stat_t".into(), json!(state_topic));
            obj.insert("cmd_t".into(), json!(cmd_topic));
            obj.insert("brightness".into(), json!(true));
            obj.insert("color_mode".into(), json!(true));
            obj.insert("supported_color_modes".into(), json!(["rgb", "color_temp"]));
            obj.insert("min_kelvin".into(), json!(MIN_COLOR_TEMP_KELVIN));
            obj.insert("max_kelvin".into(), json!(MAX_COLOR_TEMP_KELVIN));
        }
        EntityPlatform::Select => {
            obj.insert("p".into(), json!("select"));
            obj.insert("stat_t".into(), json!(state_topic));
            obj.insert("cmd_t".into(), json!(cmd_topic));
            obj.insert("ops".into(), json!(ent.options));
        }
        EntityPlatform::Number => {
            obj.insert("p".into(), json!("number"));
            obj.insert("stat_t".into(), json!(state_topic));
            obj.insert("cmd_t".into(), json!(cmd_topic));
            if let Some(range) = ent.number_range {
                obj.insert("min".into(), json!(range.min));
                obj.insert("max".into(), json!(range.max));
                obj.insert("step".into(), json!(range.step));
            }
            if let Some(unit) = &ent.unit_of_measurement {
                obj.insert("unit_of_meas".into(), json!(unit));
            }
        }
        EntityPlatform::Switch => {
            obj.insert("p".into(), json!("switch"));
            obj.insert("stat_t".into(), json!(state_topic));
            obj.insert("cmd_t".into(), json!(cmd_topic));
            obj.insert("pl_on".into(), json!("ON"));
            obj.insert("pl_off".into(), json!("OFF"));
        }
        EntityPlatform::Button => {
            obj.insert("p".into(), json!("button"));
            obj.insert("cmd_t".into(), json!(cmd_topic));
            obj.insert("pl_prs".into(), json!("PRESS"));
        }
        EntityPlatform::Update => {
            obj.insert("p".into(), json!("update"));
            obj.insert("stat_t".into(), json!(state_topic));
            obj.insert("cmd_t".into(), json!(cmd_topic));
            obj.insert("pl_inst".into(), json!("INSTALL"));
        }
    }

    Some(Value::Object(obj))
}

async fn publish_states(client: &AsyncClient, base_topic: &str, states: &[EntityState]) {
    for st in states {
        let topic = format!("{base_topic}/state/{}", st.id);
        let payload = match &st.value {
            EntityValue::Float(v) => format!("{v}"),
            EntityValue::Int(v) => format!("{v}"),
            EntityValue::Bool(b) => if *b { "ON" } else { "OFF" }.to_string(),
            EntityValue::Text(t) => t.clone(),
            EntityValue::Light(l) => {
                let mut body = json!({
                    "state": if l.is_on { "ON" } else { "OFF" },
                    "brightness": l.brightness,
                });
                match l.color_mode {
                    ctrlmate_protocol::LightColorMode::Rgb => {
                        body["color_mode"] = json!("rgb");
                        body["color"] = json!({"r": l.rgb.r, "g": l.rgb.g, "b": l.rgb.b});
                    }
                    ctrlmate_protocol::LightColorMode::ColorTemp => {
                        body["color_mode"] = json!("color_temp");
                        body["color_temp_kelvin"] = json!(l.color_temp_kelvin);
                    }
                }
                body.to_string()
            }
            EntityValue::Update(u) => json!({
                "installed_version": u.installed_version,
                "latest_version": u.latest_version,
                "title": "OS Updates",
                "in_progress": u.in_progress,
                "release_summary": u.pending_titles.join("\n"),
            })
            .to_string(),
            EntityValue::Unavailable => continue,
        };

        let _ = client.publish(topic, QoS::AtMostOnce, false, payload).await;
    }
}

async fn handle_mqtt_command(state: &Arc<AppState>, entity_id: &str, payload: &str) {
    let platform = {
        let manifest = state.latest_manifest.read().await;
        manifest
            .entities
            .iter()
            .find(|e| e.id == entity_id)
            .map(|e| e.platform)
    };

    let cmd = match platform {
        Some(EntityPlatform::Light) => {
            let Ok(v) = serde_json::from_str::<Value>(payload) else {
                warn!("Invalid MQTT JSON light payload for {entity_id}: {payload}");
                return;
            };
            let is_on = v
                .get("state")
                .and_then(Value::as_str)
                .map(|s| s.eq_ignore_ascii_case("ON"));
            let brightness = v
                .get("brightness")
                .and_then(Value::as_u64)
                .map(|b| b.min(255) as u8);
            let rgb = v.get("color").and_then(|c| {
                Some(RgbColor {
                    r: c.get("r")?.as_u64()?.min(255) as u8,
                    g: c.get("g")?.as_u64()?.min(255) as u8,
                    b: c.get("b")?.as_u64()?.min(255) as u8,
                })
            });
            let color_temp_kelvin = v
                .get("color_temp_kelvin")
                .and_then(Value::as_u64)
                .map(|k| k.clamp(u64::from(MIN_COLOR_TEMP_KELVIN), u64::from(MAX_COLOR_TEMP_KELVIN)) as u16)
                .or_else(|| {
                    v.get("color_temp")
                        .and_then(Value::as_u64)
                        .filter(|&mired| mired > 0)
                        .map(|mired| {
                            (1_000_000 / mired)
                                .clamp(u64::from(MIN_COLOR_TEMP_KELVIN), u64::from(MAX_COLOR_TEMP_KELVIN))
                                as u16
                        })
                });
            let transition_ms = v
                .get("transition")
                .and_then(Value::as_f64)
                .map(|sec| (sec.max(0.0) * 1000.0).round() as u32);
            ControlCommand::SetLight {
                entity_id: entity_id.into(),
                is_on,
                brightness,
                rgb,
                color_temp_kelvin,
                transition_ms,
            }
        }
        Some(EntityPlatform::Select) => ControlCommand::SetSelect {
            entity_id: entity_id.into(),
            option: payload.trim().into(),
        },
        Some(EntityPlatform::Number) => match payload.trim().parse::<f64>() {
            Ok(val) => ControlCommand::SetNumber {
                entity_id: entity_id.into(),
                value: val,
            },
            Err(_) => return,
        },
        Some(EntityPlatform::Switch) => ControlCommand::SetSwitch {
            entity_id: entity_id.into(),
            state: payload.trim().eq_ignore_ascii_case("ON") || payload.trim() == "1",
        },
        Some(EntityPlatform::Button) => ControlCommand::PressButton {
            entity_id: entity_id.into(),
        },
        Some(EntityPlatform::Update) if payload.trim().eq_ignore_ascii_case("INSTALL") => {
            ControlCommand::InstallOsUpdates
        }
        _ => return,
    };

    if let Err(err) = state.execute_command(cmd, true).await {
        warn!("MQTT command rejected for {entity_id}: {err}");
    }
}

fn sanitize_id(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}
