"""Ctrlmate Home Assistant 2026.11 Integration."""

from __future__ import annotations

from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant

from .const import DOMAIN, PLATFORMS
from .coordinator import CtrlmateCoordinator

type CtrlmateConfigEntry = ConfigEntry[CtrlmateCoordinator]


async def async_setup_entry(hass: HomeAssistant, entry: CtrlmateConfigEntry) -> bool:
    """Set up Ctrlmate from a config entry."""
    coordinator = CtrlmateCoordinator(hass, entry)
    entry.runtime_data = coordinator
    await coordinator.async_start()
    await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)
    return True


async def async_unload_entry(hass: HomeAssistant, entry: CtrlmateConfigEntry) -> bool:
    """Unload a Ctrlmate config entry cleanly."""
    unloaded = await hass.config_entries.async_unload_platforms(entry, PLATFORMS)
    await entry.runtime_data.async_stop()
    return unloaded
