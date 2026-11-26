"""Select platform: cooling preset and audio output."""

from __future__ import annotations

from typing import Any

from homeassistant.components.select import SelectEntity
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import CtrlmateConfigEntry
from .const import COOLING_PRESETS
from .coordinator import CtrlmateCoordinator
from .entity import CtrlmateEntity, async_setup_manifest_platform

PARALLEL_UPDATES = 1


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    async_setup_manifest_platform(hass, entry, async_add_entities, "select", CtrlmateSelect)


class CtrlmateSelect(CtrlmateEntity, SelectEntity):
    """Option list owned by the daemon."""

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        if self.key == "cooling_preset":
            self._attr_translation_key = "cooling_preset"

    @property
    def options(self) -> list[str]:
        declared = [str(o) for o in self.descriptor.get("options", [])]
        if self.key == "cooling_preset":
            # full_speed is the emergency state, not a user preset.
            return [o for o in COOLING_PRESETS if o in declared] or list(COOLING_PRESETS)
        return declared

    @property
    def current_option(self) -> str | None:
        value = self._raw_value()
        return value if isinstance(value, str) and value in self.options else None

    async def async_select_option(self, option: str) -> None:
        if self.key == "cooling_preset":
            command = {
                "action": "set_cooling_preset",
                "entity_id": self.key,
                "preset": option,
            }
        else:
            command = {"action": "set_select", "entity_id": self.key, "option": option}
        await self.coordinator.async_send_command(command)
