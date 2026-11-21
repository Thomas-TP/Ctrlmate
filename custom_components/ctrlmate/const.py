"""Constants for the Ctrlmate Home Assistant 2026.11 integration."""

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
