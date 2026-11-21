"""Binary sensor platform for Ctrlmate (connectivity, mic/webcam privacy, thermal failsafe)."""

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
from .entity import CtrlmateEntity

BINARY_CLASS_MAP: dict[str, BinarySensorDeviceClass] = {
    "connectivity": BinarySensorDeviceClass.CONNECTIVITY,
    "running": BinarySensorDeviceClass.RUNNING,
    "lock": BinarySensorDeviceClass.LOCK,
    "problem": BinarySensorDeviceClass.PROBLEM,
    "safety": BinarySensorDeviceClass.SAFETY,
    "occupancy": BinarySensorDeviceClass.OCCUPANCY,
}


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    coordinator = entry.runtime_data
    added: set[str] = set()

    # Always expose a connectivity binary sensor that remains available even when the PC is offline.
    async_add_entities([CtrlmateConnectivitySensor(coordinator)])

    def _sync_entities() -> None:
        new_entities = []
        for desc in coordinator.manifest.get("entities", []):
            if desc.get("platform") != "binary_sensor" or not desc.get("exposed", True):
                continue
            eid = desc["id"]
            if eid not in added:
                added.add(eid)
                new_entities.append(CtrlmateBinarySensor(coordinator, desc))
        if new_entities:
            async_add_entities(new_entities)

    _sync_entities()
    entry.async_on_unload(coordinator.add_manifest_listener(_sync_entities))


class CtrlmateConnectivitySensor(CtrlmateEntity, BinarySensorEntity):
    """Tracks whether the PC daemon is currently online."""

    _attr_device_class = BinarySensorDeviceClass.CONNECTIVITY

    def __init__(self, coordinator: CtrlmateCoordinator) -> None:
        super().__init__(
            coordinator,
            {"id": "pc_online", "name": "Online", "platform": "binary_sensor"},
        )

    @property
    def available(self) -> bool:
        return True

    @property
    def is_on(self) -> bool:
        return self.coordinator.connected


class CtrlmateBinarySensor(CtrlmateEntity, BinarySensorEntity):
    """Dynamic binary sensor from Ctrlmate (mic, webcam, thermal failsafe, etc.)."""

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        dc = descriptor.get("device_class")
        if dc in BINARY_CLASS_MAP:
            self._attr_device_class = BINARY_CLASS_MAP[dc]

    @property
    def is_on(self) -> bool | None:
        val = self._raw_value()
        return bool(val) if val is not None else None
