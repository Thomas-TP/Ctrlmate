"""Notify platform: desktop notifications (toast on Windows, libnotify on Linux)."""

from __future__ import annotations

from homeassistant.components.notify import NotifyEntity, NotifyEntityFeature
from homeassistant.core import HomeAssistant
from homeassistant.helpers.device_registry import DeviceInfo
from homeassistant.helpers.entity_platform import AddEntitiesCallback
from homeassistant.helpers.update_coordinator import CoordinatorEntity

from . import CtrlmateConfigEntry
from .const import DOMAIN
from .coordinator import CtrlmateCoordinator

PARALLEL_UPDATES = 1


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    async_add_entities([CtrlmateNotifyEntity(entry.runtime_data)])


class CtrlmateNotifyEntity(CoordinatorEntity[CtrlmateCoordinator], NotifyEntity):
    """Show a notification in the PC user's session.

    A disabled ``notifications`` permission or a missing agent surfaces as a
    translated error on send instead of hiding the entity.
    """

    _attr_has_entity_name = True
    _attr_translation_key = "desktop_notify"
    _attr_supported_features = NotifyEntityFeature.TITLE

    def __init__(self, coordinator: CtrlmateCoordinator) -> None:
        super().__init__(coordinator)
        self._attr_unique_id = f"{coordinator.device_id}_desktop_notify"
        self._attr_device_info = DeviceInfo(identifiers={(DOMAIN, coordinator.device_id)})

    @property
    def available(self) -> bool:
        return self.coordinator.connected

    async def async_send_message(self, message: str, title: str | None = None) -> None:
        await self.coordinator.async_send_command(
            {
                "action": "send_notification",
                "title": title or "Home Assistant",
                "message": message,
            }
        )
