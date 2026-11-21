"""Ctrlmate Home Assistant 2026.11 Integration."""

from __future__ import annotations

from homeassistant.config_entries import ConfigEntry
from homeassistant.core import Event, HomeAssistant, callback
from homeassistant.helpers import device_registry as dr
from homeassistant.helpers.device_registry import (
    CONNECTION_NETWORK_MAC,
    EVENT_DEVICE_REGISTRY_UPDATED,
    EventDeviceRegistryUpdatedData,
)

from .const import DOMAIN, PLATFORMS
from .coordinator import CtrlmateCoordinator

type CtrlmateConfigEntry = ConfigEntry[CtrlmateCoordinator]


async def async_setup_entry(hass: HomeAssistant, entry: CtrlmateConfigEntry) -> bool:
    """Set up Ctrlmate from a config entry."""
    coordinator = CtrlmateCoordinator(hass, entry)
    entry.runtime_data = coordinator
    await coordinator.async_start()

    # Pre-register the parent PC device so HA 2026.11 ChildDeviceInfo links cleanly
    # and the setup wizard only asks for the Area (room) once for the PC.
    device_reg = dr.async_get(hass)
    host = coordinator.manifest.get("host", {})
    connections = (
        {(CONNECTION_NETWORK_MAC, coordinator.mac_address)}
        if coordinator.mac_address
        else set()
    )
    parent_device = device_reg.async_get_or_create(
        config_entry_id=entry.entry_id,
        identifiers={(DOMAIN, coordinator.device_id)},
        connections=connections,
        name=host.get("name", entry.title),
        manufacturer=host.get("manufacturer", "Ctrlmate"),
        model=host.get("model", "PC"),
        sw_version=host.get("sw_version"),
    )
    coordinator.parent_device_id = parent_device.id

    # Automatically mirror the parent PC's Area onto all component child devices
    # whenever the user assigns or changes the PC's room.
    @callback
    def _sync_child_areas(event: Event[EventDeviceRegistryUpdatedData]) -> None:
        if event.data.get("action") != "update":
            return
        if event.data.get("device_id") != coordinator.parent_device_id:
            return
        changes = event.data.get("changes", {})
        if "area_id" not in changes:
            return
        parent = device_reg.async_get(coordinator.parent_device_id)
        if parent is None:
            return
        for dev in dr.async_entries_for_config_entry(device_reg, entry.entry_id):
            if dev.id != parent.id and dev.area_id != parent.area_id:
                device_reg.async_update_device(dev.id, area_id=parent.area_id)

    entry.async_on_unload(
        hass.bus.async_listen(EVENT_DEVICE_REGISTRY_UPDATED, _sync_child_areas)
    )

    await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)
    return True


async def async_unload_entry(hass: HomeAssistant, entry: CtrlmateConfigEntry) -> bool:
    """Unload a Ctrlmate config entry cleanly."""
    unloaded = await hass.config_entries.async_unload_platforms(entry, PLATFORMS)
    await entry.runtime_data.async_stop()
    return unloaded
