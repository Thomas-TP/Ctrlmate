"""Button platform: PC actions, user scripts and Wake-on-LAN."""

from __future__ import annotations

from homeassistant.components.button import ButtonEntity
from homeassistant.core import HomeAssistant
from homeassistant.helpers.device_registry import DeviceInfo
from homeassistant.helpers.entity_platform import AddEntitiesCallback
from homeassistant.helpers.update_coordinator import CoordinatorEntity

from . import CtrlmateConfigEntry
from .const import DOMAIN
from .coordinator import CtrlmateCoordinator
from .entity import CtrlmateEntity, async_setup_manifest_platform

PARALLEL_UPDATES = 1


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    # Wake-on-LAN must exist (and work) while the PC is off, so it does not depend
    # on the manifest or on the connection.
    async_add_entities([CtrlmateWakeButton(entry.runtime_data)])
    async_setup_manifest_platform(hass, entry, async_add_entities, "button", CtrlmateButton)


class CtrlmateButton(CtrlmateEntity, ButtonEntity):
    """Stateless action executed on the PC."""

    @property
    def available(self) -> bool:
        return self.coordinator.connected

    async def async_press(self) -> None:
        await self.coordinator.async_send_command({"action": "press_button", "entity_id": self.key})


class CtrlmateWakeButton(CoordinatorEntity[CtrlmateCoordinator], ButtonEntity):
    """Send a Wake-on-LAN magic packet."""

    _attr_has_entity_name = True
    _attr_translation_key = "wake_on_lan"

    def __init__(self, coordinator: CtrlmateCoordinator) -> None:
        super().__init__(coordinator)
        self._attr_unique_id = f"{coordinator.device_id}_wake_on_lan"
        self._attr_device_info = DeviceInfo(identifiers={(DOMAIN, coordinator.device_id)})

    @property
    def available(self) -> bool:
        return self.coordinator.mac_address is not None

    async def async_press(self) -> None:
        await self.coordinator.async_wake_pc()
