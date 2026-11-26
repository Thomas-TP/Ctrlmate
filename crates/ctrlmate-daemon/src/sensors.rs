//! Honest sensors: for every sensor the daemon normally expects, report whether it is available
//! and, when it is not, *why* (architecture contract section 5).

use ctrlmate_protocol::{
    DeviceCategory, DeviceManifest, EntityState, EntityValue, SecurityMode, SensorReason,
    SensorStatus, SoftwareConflict,
};
use std::collections::BTreeMap;

/// Context needed to explain a missing sensor.
pub struct SensorContext<'a> {
    pub states: &'a [EntityState],
    pub manifest: &'a DeviceManifest,
    pub mode: SecurityMode,
    pub deep_bridge_active: bool,
    pub conflicts: &'a [SoftwareConflict],
    pub windows: bool,
}

fn anti_cheat_running(conflicts: &[SoftwareConflict]) -> bool {
    conflicts.iter().any(|c| {
        c.running && (c.category.eq_ignore_ascii_case("anti-cheat") || c.id.ends_with("_ac"))
    })
}

/// Reason for a sensor that only the deep-hardware path can provide.
fn deep_reason(ctx: &SensorContext<'_>) -> SensorReason {
    match ctx.mode {
        SecurityMode::DeepHardwarePawnIo => {
            if ctx.deep_bridge_active {
                // Bridge is up but the value is missing: the board does not expose it.
                SensorReason::NotPresent
            } else if anti_cheat_running(ctx.conflicts) {
                SensorReason::AntiCheatActive
            } else {
                SensorReason::Error
            }
        }
        SecurityMode::ZeroKernelSafe => {
            if ctx.windows {
                SensorReason::RequiresDeepHardware
            } else {
                SensorReason::NotPresent
            }
        }
    }
}

fn has_value(states: &[EntityState], id: &str) -> bool {
    states
        .iter()
        .any(|s| s.id == id && s.value != EntityValue::Unavailable)
}

fn has_child(manifest: &DeviceManifest, cat: DeviceCategory) -> bool {
    manifest.child_devices.iter().any(|c| c.category == cat)
}

/// Builds the `sensor_status` map of the snapshot.
pub fn compute(ctx: &SensorContext<'_>) -> BTreeMap<String, SensorStatus> {
    let mut out = BTreeMap::new();
    let mut add = |id: &str, missing: SensorReason| {
        let st = if has_value(ctx.states, id) {
            SensorStatus::available()
        } else {
            SensorStatus::missing(missing)
        };
        out.insert(id.to_string(), st);
    };

    // CPU temperature: native on Linux; on Windows only through the deep-hardware path.
    let cpu_temp_reason = if ctx.windows {
        deep_reason(ctx)
    } else {
        SensorReason::NotPresent
    };
    add("cpu_0_temperature", cpu_temp_reason);
    add("cpu_0_power", deep_reason(ctx));
    add("memory_0_temperature", deep_reason(ctx));
    add("motherboard_0_vrm_temperature", deep_reason(ctx));
    add("motherboard_0_chipset_temperature", deep_reason(ctx));

    let gpu_reason = if has_child(ctx.manifest, DeviceCategory::Gpu) {
        SensorReason::Error
    } else {
        SensorReason::NotPresent
    };
    add("gpu_0_temperature", gpu_reason);

    let cooler_reason = if has_child(ctx.manifest, DeviceCategory::Cooler) {
        SensorReason::Error
    } else {
        SensorReason::NotPresent
    };
    add("cooler_0_liquid_temperature", cooler_reason);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::fake::{spawn_fake, state};
    use ctrlmate_protocol::ChildDeviceInfo;

    async fn manifest() -> DeviceManifest {
        let (hw, _) = spawn_fake();
        hw.build_manifest(vec![]).await.unwrap()
    }

    fn conflict(id: &str, category: &str, running: bool) -> SoftwareConflict {
        SoftwareConflict {
            id: id.into(),
            name: id.into(),
            category: category.into(),
            running,
            impact: String::new(),
        }
    }

    #[tokio::test]
    async fn windows_zero_kernel_has_no_cpu_temperature_and_says_why() {
        let m = manifest().await;
        let states = vec![state("gpu_0_temperature", EntityValue::Float(50.0))];
        let ctx = SensorContext {
            states: &states,
            manifest: &m,
            mode: SecurityMode::ZeroKernelSafe,
            deep_bridge_active: false,
            conflicts: &[],
            windows: true,
        };
        let s = compute(&ctx);
        assert_eq!(
            s["cpu_0_temperature"],
            SensorStatus::missing(SensorReason::RequiresDeepHardware)
        );
        assert_eq!(s["gpu_0_temperature"], SensorStatus::available());
        assert_eq!(
            s["cooler_0_liquid_temperature"],
            SensorStatus::missing(SensorReason::NotPresent)
        );
    }

    #[tokio::test]
    async fn anti_cheat_pauses_the_deep_path() {
        let m = manifest().await;
        let conflicts = vec![conflict("faceit_ac", "Anti-Cheat", true)];
        let ctx = SensorContext {
            states: &[],
            manifest: &m,
            mode: SecurityMode::DeepHardwarePawnIo,
            deep_bridge_active: false,
            conflicts: &conflicts,
            windows: true,
        };
        assert_eq!(
            compute(&ctx)["cpu_0_temperature"],
            SensorStatus::missing(SensorReason::AntiCheatActive)
        );
    }

    #[tokio::test]
    async fn deep_mode_without_bridge_or_anticheat_is_an_error() {
        let m = manifest().await;
        let ctx = SensorContext {
            states: &[],
            manifest: &m,
            mode: SecurityMode::DeepHardwarePawnIo,
            deep_bridge_active: false,
            conflicts: &[conflict("nzxt_cam", "AIO & RGB", true)],
            windows: true,
        };
        assert_eq!(
            compute(&ctx)["memory_0_temperature"],
            SensorStatus::missing(SensorReason::Error)
        );
    }

    #[tokio::test]
    async fn present_hardware_with_missing_value_is_an_error_not_absent() {
        let mut m = manifest().await;
        m.child_devices.push(ChildDeviceInfo {
            id: "gpu_0".into(),
            name: "GPU".into(),
            category: DeviceCategory::Gpu,
            manufacturer: None,
            model: None,
            hw_version: None,
        });
        let ctx = SensorContext {
            states: &[state("gpu_0_temperature", EntityValue::Unavailable)],
            manifest: &m,
            mode: SecurityMode::ZeroKernelSafe,
            deep_bridge_active: false,
            conflicts: &[],
            windows: false,
        };
        let s = compute(&ctx);
        assert_eq!(
            s["gpu_0_temperature"],
            SensorStatus::missing(SensorReason::Error)
        );
        assert_eq!(
            s["cpu_0_temperature"],
            SensorStatus::missing(SensorReason::NotPresent)
        );
    }
}
