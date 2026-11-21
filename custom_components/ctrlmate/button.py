"""Button platform for Ctrlmate (Power, Display Off, Scripts, and always-available Wake-on-LAN)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.button import ButtonEntity
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import CtrlmateConfigEntry
from .coordinator import CtrlmateCoordinator
from .entity import CtrlmateEntity


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    coordinator = entry.runtime_data
    added: set[str] = set()

    # Wake-on-LAN button stays available even while the PC is powered off.
    if coordinator.mac_address:
        async_add_entities([CtrlmateWakeButton(coordinator)])

    def _sync_entities() -> None:
        new_entities = []
        for desc in coordinator.manifest.get("entities", []):
            if desc.get("platform") != "button" or not desc.get("exposed", True):
                continue
            eid = desc["id"]
            if eid not in added:
                added.add(eid)
                new_entities.append(CtrlmateActionButton(coordinator, desc))
        if new_entities:
            async_add_entities(new_entities)

    _sync_entities()
    entry.async_on_unload(coordinator.add_manifest_listener(_sync_entities))


class CtrlmateWakeButton(CtrlmateEntity, ButtonEntity):
    """Wake-on-LAN button executed locally from Home Assistant."""

    _attr_icon = "mdi:lan-connect"

    def __init__(self, coordinator: CtrlmateCoordinator) -> None:
        super().__init__(
            coordinator,
            {"id": "wake_on_lan", "name": "Wake on LAN", "platform": "button"},
        )

    @property
    def available(self) -> bool:
        return bool(self.coordinator.mac_address)

    async def async_press(self) -> None:
        await self.coordinator.async_wake_pc()


class CtrlmateActionButton(CtrlmateEntity, ButtonEntity):
    """Remote OS action or allowlisted user script button."""

    async def async_press(self) -> None:
        await self.coordinator.async_send_command(
            {"action": "press_button", "entity_id": self.key}
        )
