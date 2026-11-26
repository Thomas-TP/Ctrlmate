//! Detects running OEM hardware / RGB software that contends for the same buses, plus the
//! anti-cheats of [`crate::anticheat`] (single source of truth for those).

use crate::anticheat::{self, AntiCheat};
use ctrlmate_protocol::SoftwareConflict;

struct KnownConflict<'a> {
    id: &'a str,
    name: &'a str,
    category: &'a str,
    /// Lower-case process names without `.exe`; entries are matched exactly.
    processes: &'a [&'a str],
    impact: &'a str,
}

const KNOWN_CONFLICTS: &[KnownConflict<'_>] = &[
    KnownConflict {
        id: "gigabyte_gcc",
        name: "GIGABYTE Control Center",
        category: "RGB & Fans",
        processes: &["gcc", "gbt_rgb", "gigabytecontrolcenter", "rgbfusion"],
        impact: "Can lock the motherboard ITE RGB controller and fan headers.",
    },
    KnownConflict {
        id: "nzxt_cam",
        name: "NZXT CAM",
        category: "AIO & RGB",
        processes: &["nzxt cam", "cam_helper"],
        impact: "Holds exclusive USB HID access to NZXT Kraken coolers and hubs.",
    },
    KnownConflict {
        id: "asus_armoury",
        name: "ASUS Armoury Crate / Aura",
        category: "RGB & Sensors",
        processes: &[
            "armourycrate.service",
            "armourycrate.usersessionhelper",
            "lightingservice",
            "asusframework",
        ],
        impact: "Can contend over SMBus/Aura RGB controllers and VGA LEDs.",
    },
    KnownConflict {
        id: "corsair_icue",
        name: "Corsair iCUE",
        category: "AIO & RGB",
        processes: &["icue", "corsair.service"],
        impact: "Can cause USB HID contention with Corsair hubs and DRAM RGB.",
    },
    KnownConflict {
        id: "samsung_magician",
        name: "Samsung Magician",
        category: "Storage",
        processes: &["samsungmagician", "magicianservice"],
        impact: "Proprietary manager for Samsung 990 PRO Heatsink LED (no conflict with SMART telemetry).",
    },
];

fn normalize(name: &str) -> String {
    let l = name.trim().to_ascii_lowercase();
    l.strip_suffix(".exe").unwrap_or(&l).to_string()
}

/// Builds the conflict list from process names and the already-detected anti-cheats.
pub fn conflicts_from(
    names: &[String],
    anti_cheats: &[&'static AntiCheat],
) -> Vec<SoftwareConflict> {
    let lower: Vec<String> = names.iter().map(|n| normalize(n)).collect();
    let mut out: Vec<SoftwareConflict> = KNOWN_CONFLICTS
        .iter()
        .filter(|k| lower.iter().any(|n| k.processes.contains(&n.as_str())))
        .map(|k| SoftwareConflict {
            id: k.id.into(),
            name: k.name.into(),
            category: k.category.into(),
            running: true,
            impact: k.impact.into(),
        })
        .collect();
    out.extend(anti_cheats.iter().map(|a| anticheat::to_conflict(a)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_software_and_anti_cheats_are_reported() {
        let names = vec![
            "NZXT CAM.exe".to_string(),
            "chrome.exe".to_string(),
            "vgc.exe".to_string(),
        ];
        let ac = anticheat::match_anti_cheats(names.iter().map(String::as_str), &[]);
        let c = conflicts_from(&names, &ac);
        let ids: Vec<_> = c.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["nzxt_cam", "vanguard_ac"]);
        assert!(c.iter().all(|x| x.running));
    }

    #[test]
    fn substrings_do_not_match() {
        // The old matcher used `contains`: "gigabyte" matched any Gigabyte-named helper process.
        let names = vec![
            "gigabyte_updater.exe".to_string(),
            "myicuecompanion.exe".to_string(),
        ];
        assert!(conflicts_from(&names, &[]).is_empty());
    }
}
