"""Diagnostics for Ctrlmate (secrets and identifiers redacted)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.diagnostics import async_redact_data
from homeassistant.const import CONF_HOST
from homeassistant.core import HomeAssistant

from . import CtrlmateConfigEntry
from .const import (
    CONF_CERT_FINGERPRINT,
    CONF_CLIENT_ID,
    CONF_DEVICE_ID,
    CONF_MAC_ADDRESS,
    CONF_TOKEN,
    CONF_WOL_BROADCAST,
)

TO_REDACT = {
    CONF_TOKEN,
    CONF_CERT_FINGERPRINT,
    "cert_fingerprint",
    CONF_MAC_ADDRESS,
    "mac_address",
    CONF_HOST,
    CONF_CLIENT_ID,
    CONF_DEVICE_ID,
    "device_id",
    CONF_WOL_BROADCAST,
    "hostname",
    "active_application",
    "pending_titles",
}


async def async_get_config_entry_diagnostics(
    hass: HomeAssistant, entry: CtrlmateConfigEntry
) -> dict[str, Any]:
    """Return redacted diagnostics for a config entry."""
    coordinator = entry.runtime_data
    manifest = coordinator.manifest
    host = dict(manifest.get("host", {}))
    host.pop("name", None)

    return {
        "entry": {
            "title": "**REDACTED**",
            "data": async_redact_data(dict(entry.data), TO_REDACT),
            "options": async_redact_data(dict(entry.options), TO_REDACT),
        },
        "connected": coordinator.connected,
        "protocol_version": manifest.get("protocol_version"),
        "host": async_redact_data(host, TO_REDACT),
        "child_devices": [
            {k: v for k, v in c.items() if k != "name"} for c in manifest.get("child_devices", [])
        ],
        "entities": [
            {
                key: desc.get(key)
                for key in ("id", "platform", "child_device_id", "device_class", "exposed")
            }
            for desc in manifest.get("entities", [])
        ],
        "states": {
            ent_id: async_redact_data(state, TO_REDACT)
            for ent_id, state in coordinator.data.items()
        },
    }
