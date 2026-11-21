"""Update platform for Ctrlmate (Windows Update WUA & Linux PackageKit/fwupd)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.update import UpdateEntity, UpdateEntityFeature
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import CtrlmateConfigEntry
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
            if desc.get("platform") != "update" or not desc.get("exposed", True):
                continue
            eid = desc["id"]
            if eid not in added:
                added.add(eid)
                new_entities.append(CtrlmateUpdateEntity(coordinator, desc))
        if new_entities:
            async_add_entities(new_entities)

    _sync_entities()
    entry.async_on_unload(coordinator.add_manifest_listener(_sync_entities))


class CtrlmateUpdateEntity(CtrlmateEntity, UpdateEntity):
    """Native Home Assistant Update entity for pending Windows/Linux updates."""

    _attr_supported_features = UpdateEntityFeature.INSTALL

    def _update_state(self) -> dict[str, Any]:
        val = self._raw_value()
        return val if isinstance(val, dict) else {}

    @property
    def installed_version(self) -> str | None:
        return self._update_state().get("installed_version")

    @property
    def latest_version(self) -> str | None:
        st = self._update_state()
        pending = int(st.get("pending_count", 0))
        if pending > 0:
            return f"{st.get('installed_version', 'OS')} (+{pending} update{'s' if pending > 1 else ''})"
        return st.get("installed_version")

    @property
    def in_progress(self) -> bool:
        return bool(self._update_state().get("in_progress", False))

    @property
    def release_summary(self) -> str | None:
        titles = self._update_state().get("pending_titles") or []
        if not titles:
            return None
        return "\n".join(f"- {t}" for t in titles[:10])

    async def async_install(self, version: str | None, backup: bool, **kwargs: Any) -> None:
        await self.coordinator.async_send_command({"action": "install_os_updates"})
