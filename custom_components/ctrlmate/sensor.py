"""Sensor platform for Ctrlmate (native HA °C / °F conversion via SensorDeviceClass.TEMPERATURE)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.sensor import (
    SensorDeviceClass,
    SensorEntity,
    SensorStateClass,
)
from homeassistant.const import UnitOfTemperature
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import CtrlmateConfigEntry
from .coordinator import CtrlmateCoordinator
from .entity import CtrlmateEntity

DEVICE_CLASS_MAP: dict[str, SensorDeviceClass] = {
    "temperature": SensorDeviceClass.TEMPERATURE,
    "power": SensorDeviceClass.POWER,
    "frequency": SensorDeviceClass.FREQUENCY,
    "data_size": SensorDeviceClass.DATA_SIZE,
    "voltage": SensorDeviceClass.VOLTAGE,
    "duration": SensorDeviceClass.DURATION,
}

STATE_CLASS_MAP: dict[str, SensorStateClass] = {
    "measurement": SensorStateClass.MEASUREMENT,
    "total": SensorStateClass.TOTAL,
    "total_increasing": SensorStateClass.TOTAL_INCREASING,
}


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    coordinator = entry.runtime_data
    added: set[str] = set()

    def _sync_entities() -> None:
        new_entities = []
        for desc in coordinator.manifest.get("entities", []):
            if desc.get("platform") != "sensor" or not desc.get("exposed", True):
                continue
            eid = desc["id"]
            if eid not in added:
                added.add(eid)
                new_entities.append(CtrlmateSensor(coordinator, desc))
        if new_entities:
            async_add_entities(new_entities)

    _sync_entities()
    entry.async_on_unload(coordinator.add_manifest_listener(_sync_entities))


class CtrlmateSensor(CtrlmateEntity, SensorEntity):
    """Numeric or text telemetry sensor from Ctrlmate."""

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        dc = descriptor.get("device_class")
        if dc in DEVICE_CLASS_MAP:
            self._attr_device_class = DEVICE_CLASS_MAP[dc]
        sc = descriptor.get("state_class")
        if sc in STATE_CLASS_MAP:
            self._attr_state_class = STATE_CLASS_MAP[sc]
        unit = descriptor.get("unit_of_measurement")
        if dc == "temperature":
            self._attr_native_unit_of_measurement = UnitOfTemperature.CELSIUS
        elif unit:
            self._attr_native_unit_of_measurement = unit
        if (prec := descriptor.get("suggested_display_precision")) is not None:
            self._attr_suggested_display_precision = int(prec)

    @property
    def native_value(self) -> Any:
        return self._raw_value()
