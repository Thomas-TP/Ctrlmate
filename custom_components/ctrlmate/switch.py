"""Switch platform (audio mute)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.switch import SwitchEntity
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import CtrlmateConfigEntry
from .entity import CtrlmateEntity, async_setup_manifest_platform

PARALLEL_UPDATES = 1


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    async_setup_manifest_platform(hass, entry, async_add_entities, "switch", CtrlmateSwitch)


class CtrlmateSwitch(CtrlmateEntity, SwitchEntity):
    """Boolean setting on the PC."""

    @property
    def is_on(self) -> bool | None:
        value = self._raw_value()
        return bool(value) if value is not None else None

    async def _set(self, state: bool) -> None:
        await self.coordinator.async_send_command(
            {"action": "set_switch", "entity_id": self.key, "state": state}
        )

    async def async_turn_on(self, **kwargs: Any) -> None:
        await self._set(True)

    async def async_turn_off(self, **kwargs: Any) -> None:
        await self._set(False)
