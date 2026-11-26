"""Binary sensor platform for Ctrlmate."""

from __future__ import annotations

from typing import Any

from homeassistant.components.binary_sensor import (
    BinarySensorDeviceClass,
    BinarySensorEntity,
)
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import CtrlmateConfigEntry
from .coordinator import CtrlmateCoordinator
from .entity import CtrlmateEntity, async_setup_manifest_platform
import contextlib

PARALLEL_UPDATES = 0


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    async_setup_manifest_platform(
        hass, entry, async_add_entities, "binary_sensor", CtrlmateBinarySensor
    )


class CtrlmateBinarySensor(CtrlmateEntity, BinarySensorEntity):
    """Boolean state reported by the PC."""

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        if raw := descriptor.get("device_class"):
            with contextlib.suppress(ValueError):
                self._attr_device_class = BinarySensorDeviceClass(raw)

    @property
    def is_on(self) -> bool | None:
        value = self._raw_value()
        return bool(value) if value is not None else None
