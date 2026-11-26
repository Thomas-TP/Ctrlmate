"""Sensor platform for Ctrlmate."""

from __future__ import annotations

from typing import Any

from homeassistant.components.sensor import (
    DEVICE_CLASS_UNITS,
    SensorDeviceClass,
    SensorEntity,
    SensorStateClass,
)
from homeassistant.const import UnitOfTemperature
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import CtrlmateConfigEntry
from .coordinator import CtrlmateCoordinator
from .entity import CtrlmateEntity, async_setup_manifest_platform, update_pending_count
import contextlib

PARALLEL_UPDATES = 0

PENDING_UPDATES_DESCRIPTOR: dict[str, Any] = {
    "id": "os_pending_updates",
    "name": "Pending OS updates",
    "platform": "sensor",
    "state_class": "measurement",
    "entity_category": "diagnostic",
    "suggested_display_precision": 0,
}


def _extra(coordinator: CtrlmateCoordinator) -> list[dict[str, Any]]:
    """The pending-update count is derived from the ``os_updates`` state."""
    if (
        "os_updates" in coordinator.descriptors
        and "os_pending_updates" not in coordinator.descriptors
    ):
        return [PENDING_UPDATES_DESCRIPTOR]
    return []


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    async_setup_manifest_platform(hass, entry, async_add_entities, "sensor", CtrlmateSensor, _extra)


class CtrlmateSensor(CtrlmateEntity, SensorEntity):
    """Telemetry sensor; a missing reading is unavailable, never zero."""

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        unit = descriptor.get("unit_of_measurement")
        device_class: SensorDeviceClass | None = None
        if raw := descriptor.get("device_class"):
            try:
                device_class = SensorDeviceClass(raw)
            except ValueError:
                device_class = None
        if device_class == SensorDeviceClass.TEMPERATURE:
            unit = UnitOfTemperature.CELSIUS
        allowed = DEVICE_CLASS_UNITS.get(device_class) if device_class else None
        if device_class is not None and allowed is not None and unit not in allowed:
            device_class = None  # never register a unit HA would reject
        if device_class is not None:
            self._attr_device_class = device_class
        if unit:
            self._attr_native_unit_of_measurement = unit

        state_class = descriptor.get("state_class")
        if state_class and (unit or device_class):
            with contextlib.suppress(ValueError):
                self._attr_state_class = SensorStateClass(state_class)
        if (precision := descriptor.get("suggested_display_precision")) is not None:
            self._attr_suggested_display_precision = int(precision)

    def _raw_state(self) -> dict[str, Any] | None:
        if self.key == "os_pending_updates":
            return self.coordinator.data.get("os_updates")
        return super()._raw_state()

    @property
    def available(self) -> bool:
        if self.key == "os_pending_updates":
            value = self._raw_value()
            return self.coordinator.connected and isinstance(value, dict) and not value.get("error")
        return super().available

    @property
    def native_value(self) -> Any:
        value = self._raw_value()
        if self.key == "os_pending_updates":
            return update_pending_count(value) if isinstance(value, dict) else None
        return value
