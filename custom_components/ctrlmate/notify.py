"""Notify platform for Ctrlmate (native Windows Toast / Linux desktop notifications)."""

from __future__ import annotations

from homeassistant.components.notify import NotifyEntity, NotifyEntityFeature
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
    async_add_entities([CtrlmateNotifyEntity(entry.runtime_data)])


class CtrlmateNotifyEntity(CtrlmateEntity, NotifyEntity):
    """Send native desktop notifications to the Ctrlmate PC from Home Assistant."""

    _attr_supported_features = NotifyEntityFeature.TITLE
    _attr_icon = "mdi:message-badge-outline"

    def __init__(self, coordinator: CtrlmateCoordinator) -> None:
        super().__init__(
            coordinator,
            {"id": "desktop_notify", "name": "Notifications", "platform": "notify"},
        )

    async def async_send_message(self, message: str, title: str | None = None) -> None:
        await self.coordinator.async_send_command(
            {
                "action": "send_notification",
                "title": title or "Home Assistant",
                "message": message,
            }
        )
