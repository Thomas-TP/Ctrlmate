"""Number platform for Ctrlmate (Master Volume, numeric controls)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.number import NumberEntity, NumberMode
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

    def _sync_entities() -> None:
        new_entities = []
        for desc in coordinator.manifest.get("entities", []):
            if desc.get("platform") != "number" or not desc.get("exposed", True):
                continue
            eid = desc["id"]
            if eid not in added:
                added.add(eid)
                new_entities.append(CtrlmateNumber(coordinator, desc))
        if new_entities:
            async_add_entities(new_entities)

    _sync_entities()
    entry.async_on_unload(coordinator.add_manifest_listener(_sync_entities))


class CtrlmateNumber(CtrlmateEntity, NumberEntity):
    """Adjustable numeric control (e.g. Master Volume 0-100%)."""

    _attr_mode = NumberMode.SLIDER

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        rng = descriptor.get("number_range") or {}
        self._attr_native_min_value = float(rng.get("min", 0.0))
        self._attr_native_max_value = float(rng.get("max", 100.0))
        self._attr_native_step = float(rng.get("step", 1.0))
        if unit := descriptor.get("unit_of_measurement"):
            self._attr_native_unit_of_measurement = unit

    @property
    def native_value(self) -> float | None:
        val = self._raw_value()
        return float(val) if val is not None else None

    async def async_set_native_value(self, value: float) -> None:
        await self.coordinator.async_send_command(
            {"action": "set_number", "entity_id": self.key, "value": float(value)}
        )
