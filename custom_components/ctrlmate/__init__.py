"""The Ctrlmate integration: local push control and telemetry for a PC."""

from __future__ import annotations

from homeassistant.config_entries import ConfigEntry
from homeassistant.const import CONF_HOST, CONF_PORT
from homeassistant.core import HomeAssistant
from homeassistant.helpers import device_registry as dr

from .const import CONF_CERT_FINGERPRINT, CONF_TOKEN, DOMAIN, PLATFORMS
from .coordinator import CtrlmateCoordinator

type CtrlmateConfigEntry = ConfigEntry[CtrlmateCoordinator]

_CONNECTION_KEYS = (CONF_HOST, CONF_PORT, CONF_TOKEN, CONF_CERT_FINGERPRINT)


async def async_setup_entry(hass: HomeAssistant, entry: CtrlmateConfigEntry) -> bool:
    """Set up Ctrlmate from a config entry."""
    coordinator = CtrlmateCoordinator(hass, entry)
    entry.runtime_data = coordinator

    # Raises ConfigEntryAuthFailed when the token is already revoked; an offline PC
    # is fine (entities turn available when it connects, Wake-on-LAN keeps working).
    await coordinator.async_start()

    snapshot = {key: entry.data.get(key) for key in _CONNECTION_KEYS}

    async def _async_entry_updated(hass: HomeAssistant, entry: ConfigEntry) -> None:
        # The coordinator itself rewrites the MAC address; only reload when
        # something the connection depends on changed (reconfigure / reauth / zeroconf).
        if any(entry.data.get(key) != value for key, value in snapshot.items()):
            hass.config_entries.async_schedule_reload(entry.entry_id)

    entry.async_on_unload(entry.add_update_listener(_async_entry_updated))

    await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)
    return True


async def async_unload_entry(hass: HomeAssistant, entry: CtrlmateConfigEntry) -> bool:
    """Unload a Ctrlmate config entry."""
    unloaded = await hass.config_entries.async_unload_platforms(entry, PLATFORMS)
    if unloaded:
        await entry.runtime_data.async_stop()
    return unloaded


async def async_remove_config_entry_device(
    hass: HomeAssistant, entry: CtrlmateConfigEntry, device_entry: dr.DeviceEntry
) -> bool:
    """Allow deleting stale devices, never the PC or a component that still exists."""
    coordinator = entry.runtime_data
    keep = coordinator.expected_child_identifiers() | {(DOMAIN, coordinator.device_id)}
    return not (device_entry.identifiers & keep)
