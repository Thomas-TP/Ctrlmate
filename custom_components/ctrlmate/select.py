"""Select platform for Ctrlmate (cooling presets, audio output selector)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.select import SelectEntity
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
            if desc.get("platform") != "select" or not desc.get("exposed", True):
                continue
            eid = desc["id"]
            if eid not in added:
                added.add(eid)
                new_entities.append(CtrlmateSelect(coordinator, desc))
        if new_entities:
            async_add_entities(new_entities)

    _sync_entities()
    entry.async_on_unload(coordinator.add_manifest_listener(_sync_entities))


class CtrlmateSelect(CtrlmateEntity, SelectEntity):
    """Selectable mode or output exposed by Ctrlmate."""

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        self._attr_options = list(descriptor.get("options") or [])

    @property
    def current_option(self) -> str | None:
        val = self._raw_value()
        return str(val) if val is not None else None

    async def async_select_option(self, option: str) -> None:
        await self.coordinator.async_send_command(
            {
                "action": "set_select",
                "entity_id": self.key,
                "option": option,
            }
        )
