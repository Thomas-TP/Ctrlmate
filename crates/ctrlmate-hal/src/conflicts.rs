//! Detects running OEM hardware/RGB software or strict anti-cheats that may conflict with bus access.

use ctrlmate_protocol::SoftwareConflict;
use sysinfo::System;

struct KnownConflict<'a> {
    id: &'a str,
    name: &'a str,
    category: &'a str,
    process_patterns: &'a [&'a str],
    impact: &'a str,
}

const KNOWN_CONFLICTS: &[KnownConflict<'_>] = &[
    KnownConflict {
        id: "gigabyte_gcc",
        name: "GIGABYTE Control Center",
        category: "RGB & Fans",
        process_patterns: &["gcc.exe", "gbt_rgb", "gigabyte"],
        impact: "Can lock the motherboard ITE RGB controller and fan headers.",
    },
    KnownConflict {
        id: "nzxt_cam",
        name: "NZXT CAM",
        category: "AIO & RGB",
        process_patterns: &["nzxt cam.exe", "cam_helper"],
        impact: "Holds exclusive USB HID access to NZXT Kraken coolers and hubs.",
    },
    KnownConflict {
        id: "asus_armoury",
        name: "ASUS Armoury Crate / Aura",
        category: "RGB & Sensors",
        process_patterns: &["armourycrate", "lightingservice.exe", "asusframework"],
        impact: "Can contend over SMBus/Aura RGB controllers and VGA LEDs.",
    },
    KnownConflict {
        id: "corsair_icue",
        name: "Corsair iCUE",
        category: "AIO & RGB",
        process_patterns: &["icue.exe", "corsair.service"],
        impact: "Can cause USB HID contention with Corsair hubs and DRAM RGB.",
    },
    KnownConflict {
        id: "samsung_magician",
        name: "Samsung Magician",
        category: "Storage",
        process_patterns: &["samsungmagician", "magicianservice"],
        impact: "Proprietary manager for Samsung 990 PRO Heatsink LED (no conflict with SMART telemetry).",
    },
    KnownConflict {
        id: "faceit_ac",
        name: "FACEIT Anti-Cheat",
        category: "Anti-Cheat",
        process_patterns: &["faceitclient", "faceitservice"],
        impact: "Blocks PawnIO kernel driver; keep Ctrlmate in Zero-Kernel Anti-Cheat Safe mode.",
    },
];

pub fn detect_running_conflicts(sys: &System) -> Vec<SoftwareConflict> {
    let running_names: Vec<String> = sys
        .processes()
        .values()
        .map(|p| p.name().to_string_lossy().to_ascii_lowercase())
        .collect();

    let mut conflicts = Vec::new();
    for entry in KNOWN_CONFLICTS {
        let is_running = running_names.iter().any(|proc_name| {
            entry
                .process_patterns
                .iter()
                .any(|pat| proc_name.contains(pat))
        });
        if is_running {
            conflicts.push(SoftwareConflict {
                id: entry.id.into(),
                name: entry.name.into(),
                category: entry.category.into(),
                running: true,
                impact: entry.impact.into(),
            });
        }
    }

    conflicts
}
