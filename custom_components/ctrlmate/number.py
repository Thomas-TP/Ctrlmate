"""Number platform (master volume)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.number import NumberEntity, NumberMode
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import CtrlmateConfigEntry
from .coordinator import CtrlmateCoordinator
from .entity import CtrlmateEntity, async_setup_manifest_platform

PARALLEL_UPDATES = 1


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    async_setup_manifest_platform(hass, entry, async_add_entities, "number", CtrlmateNumber)


class CtrlmateNumber(CtrlmateEntity, NumberEntity):
    """Adjustable value with bounds from the manifest."""

    _attr_mode = NumberMode.SLIDER

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        rng = descriptor.get("number_range") or {"min": 0, "max": 100, "step": 1}
        self._attr_native_min_value = float(rng["min"])
        self._attr_native_max_value = float(rng["max"])
        self._attr_native_step = float(rng["step"])
        if unit := descriptor.get("unit_of_measurement"):
            self._attr_native_unit_of_measurement = unit

    @property
    def native_value(self) -> float | None:
        value = self._raw_value()
        return float(value) if isinstance(value, (int, float)) else None

    async def async_set_native_value(self, value: float) -> None:
        await self.coordinator.async_send_command(
            {"action": "set_number", "entity_id": self.key, "value": float(value)}
        )
