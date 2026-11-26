"""Constants for the Ctrlmate integration."""

from __future__ import annotations

from homeassistant.const import Platform

DOMAIN = "ctrlmate"

DEFAULT_PORT = 9487

CONF_DEVICE_ID = "device_id"
CONF_CLIENT_ID = "client_id"
CONF_TOKEN = "token"
CONF_CERT_FINGERPRINT = "cert_fingerprint"
CONF_MAC_ADDRESS = "mac_address"
CONF_PIN = "pin"
CONF_CONFIRM = "confirm"

# Options (Wake-on-LAN)
CONF_WOL_BROADCAST = "wol_broadcast_address"
CONF_WOL_PORT = "wol_port"
DEFAULT_WOL_PORT = 9

PLATFORMS: list[Platform] = [
    Platform.SENSOR,
    Platform.BINARY_SENSOR,
    Platform.LIGHT,
    Platform.SELECT,
    Platform.BUTTON,
    Platform.NUMBER,
    Platform.SWITCH,
    Platform.UPDATE,
    Platform.NOTIFY,
]

# Stable result codes sent by ctrlmated (docs/ARCHITECTURE.md section 4).
RESULT_CODES: frozenset[str] = frozenset(
    {
        "permission_denied",
        "no_agent",
        "agent_timeout",
        "not_supported",
        "invalid_argument",
        "hardware_error",
        "busy",
        "unavailable",
        "internal",
    }
)

# Cooling presets accepted by the daemon (``full_speed`` is an emergency state only).
COOLING_PRESETS: tuple[str, ...] = (
    "auto",
    "silent",
    "balanced",
    "performance",
    "gaming",
)

COMMAND_TIMEOUT = 12.0
CONNECT_TIMEOUT = 10.0
BACKOFF_MIN = 2.0
BACKOFF_MAX = 60.0

# Repairs issue ids
ISSUE_NO_AGENT = "no_agent"
ISSUE_SERVICE_OUTDATED = "service_outdated"

# Entity id (without child prefix) -> translation_key, used when a descriptor
# carries no translation key of its own. Anything unknown falls back to the name
# supplied by the manifest (English default), never to a guessed translation.
PARENT_TRANSLATION_KEYS: dict[str, str] = {
    "light_global": "pc_lighting",
    "cooling_preset": "cooling_preset",
    "thermal_failsafe": "thermal_failsafe",
    "active_application": "active_application",
    "gaming_mode": "gaming_mode",
    "os_updates": "os_updates",
    "os_pending_updates": "os_pending_updates",
    "reboot_required": "reboot_required",
    "microphone_in_use": "microphone_in_use",
    "webcam_in_use": "webcam_in_use",
    "master_volume": "master_volume",
    "audio_mute": "audio_mute",
    "audio_output": "audio_output",
    "check_os_updates": "check_os_updates",
    "power_shutdown": "power_shutdown",
    "power_restart": "power_restart",
    "power_sleep": "power_sleep",
    "power_hibernate": "power_hibernate",
    "power_lock": "power_lock",
    "display_off": "display_off",
}

CHILD_TRANSLATION_KEYS: dict[str, str] = {
    "load": "load",
    "frequency": "frequency",
    "temperature": "temperature",
    "power": "power",
    "usage": "usage",
    "used": "used",
    "vram_usage": "vram_usage",
    "fan_speed": "fan_speed",
    "clock_graphics": "core_clock",
    "vrm_temperature": "vrm_temperature",
    "chipset_temperature": "chipset_temperature",
    "liquid_temperature": "liquid_temperature",
    "pump_rpm": "pump_speed",
    "pump_duty": "pump_duty",
    "fan_rpm": "radiator_fan_speed",
    "fan_duty": "radiator_fan_duty",
}

# Rarely useful entities start disabled so the default dashboard stays calm.
DISABLED_BY_DEFAULT_KEYS: frozenset[str] = frozenset(
    {
        "active_application",
        "pump_duty",
        "radiator_fan_duty",
        "vrm_temperature",
        "chipset_temperature",
        "used",
        "core_clock",
        "frequency",
    }
)
