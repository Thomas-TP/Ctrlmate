"""Base entity and manifest-driven platform setup for Ctrlmate."""

from __future__ import annotations

from collections.abc import Callable
import re
from typing import TYPE_CHECKING, Any

from homeassistant.const import EntityCategory
from homeassistant.core import HomeAssistant
from homeassistant.helpers.device_registry import ChildDeviceInfo, DeviceInfo
from homeassistant.helpers.entity import Entity
from homeassistant.helpers.entity_platform import AddEntitiesCallback
from homeassistant.helpers.update_coordinator import CoordinatorEntity

from .const import (
    CHILD_TRANSLATION_KEYS,
    DISABLED_BY_DEFAULT_KEYS,
    DOMAIN,
    PARENT_TRANSLATION_KEYS,
)
from .coordinator import CtrlmateCoordinator

if TYPE_CHECKING:
    from . import CtrlmateConfigEntry


_KB_RE = re.compile(r"(?<![A-Za-z0-9])KB\d{4,}(?!\d)", re.IGNORECASE)


def update_pending_count(info: dict[str, Any]) -> int | None:
    """Pending OS update count, or None when no scan succeeded (never a fake 0)."""
    if not info or info.get("error") or info.get("checked") is False:
        return None
    count = info.get("pending_count")
    return int(count) if count is not None else None


def update_titles(info: dict[str, Any]) -> list[str]:
    """Pending update titles from either the flat or the structured wire shape."""
    if titles := info.get("pending_titles"):
        return [str(t) for t in titles]
    return [
        str(p["title"]) for p in info.get("pending") or [] if isinstance(p, dict) and "title" in p
    ]


def update_kb_ids(info: dict[str, Any]) -> list[str]:
    """KB ids: explicit list when the daemon sends it, otherwise parsed from titles."""
    explicit = info.get("kb_ids") or info.get("pending_kb_ids")
    if explicit:
        return [str(i).upper() for i in explicit]
    found: list[str] = []
    for title in update_titles(info):
        for match in _KB_RE.findall(title):
            kb = match.upper()
            if kb not in found:
                found.append(kb)
    return found


def translation_key_for(descriptor: dict[str, Any]) -> str | None:
    """Map a manifest descriptor to a translation key, or None when unknown."""
    entity_id: str = descriptor["id"]
    child = descriptor.get("child_device_id")
    if child:
        if descriptor.get("light_caps") is not None:
            return "lighting"
        prefix = f"{child}_"
        if entity_id.startswith(prefix):
            return CHILD_TRANSLATION_KEYS.get(entity_id[len(prefix) :])
        return None
    return PARENT_TRANSLATION_KEYS.get(entity_id)


class CtrlmateEntity(CoordinatorEntity[CtrlmateCoordinator]):
    """Shared base: naming, device binding and honest availability."""

    _attr_has_entity_name = True

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator)
        self._initial_descriptor = descriptor
        self.key: str = descriptor["id"]
        self._attr_unique_id = f"{coordinator.device_id}_{self.key}"

        translation_key = translation_key_for(descriptor)
        if translation_key:
            self._attr_translation_key = translation_key
        else:
            # Unknown hardware (disk, case fan, user script): use the manifest name.
            self._attr_name = descriptor.get("name") or self.key
            if icon := descriptor.get("icon"):
                self._attr_icon = icon
        if translation_key in DISABLED_BY_DEFAULT_KEYS:
            self._attr_entity_registry_enabled_default = False

        category = descriptor.get("entity_category")
        if category == "diagnostic":
            self._attr_entity_category = EntityCategory.DIAGNOSTIC
        elif category == "config":
            self._attr_entity_category = EntityCategory.CONFIG

    # ---------------------------------------------------------------- device

    @property
    def descriptor(self) -> dict[str, Any]:
        """Latest manifest descriptor for this entity."""
        return self.coordinator.descriptors.get(self.key, self._initial_descriptor)

    @property
    def device_info(self) -> DeviceInfo | ChildDeviceInfo:
        coordinator = self.coordinator
        child_id = self._initial_descriptor.get("child_device_id")
        if child_id and coordinator.parent_device_id:
            child = next(
                (
                    c
                    for c in coordinator.manifest.get("child_devices", [])
                    if c.get("id") == child_id
                ),
                None,
            )
            if child is not None:
                # Child devices inherit the parent PC's area natively.
                return ChildDeviceInfo(
                    identifiers={(DOMAIN, f"{coordinator.device_id}_{child_id}")},
                    name=child.get("name") or child_id,
                    parent_device_id=coordinator.parent_device_id,
                )
        return DeviceInfo(identifiers={(DOMAIN, coordinator.device_id)})

    # ----------------------------------------------------------------- state

    def _raw_state(self) -> dict[str, Any] | None:
        return self.coordinator.data.get(self.key)

    def _status(self) -> dict[str, Any] | None:
        status = (self._raw_state() or {}).get("status")
        return status if isinstance(status, dict) else None

    def _raw_value(self) -> Any:
        """Decoded value, or None for missing / unavailable / null readings."""
        state = self._raw_state()
        if not state:
            return None
        status = self._status()
        if status is not None and status.get("available") is False:
            return None
        value = state.get("value")
        if isinstance(value, dict) and "kind" in value:
            if value["kind"] == "unavailable":
                return None
            return value.get("value")
        return value

    @property
    def available(self) -> bool:
        return self.coordinator.connected and self._raw_value() is not None

    @property
    def extra_state_attributes(self) -> dict[str, Any] | None:
        state = self._raw_state()
        if not state:
            return None
        attrs = state.get("attributes")
        return dict(attrs) if attrs else None


def async_setup_manifest_platform(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
    platform: str,
    factory: Callable[[CtrlmateCoordinator, dict[str, Any]], Entity],
    extra: Callable[[CtrlmateCoordinator], list[dict[str, Any]]] | None = None,
) -> None:
    """Create entities for every exposed descriptor of ``platform``, now and later."""
    coordinator = entry.runtime_data
    added: set[str] = set()

    def _sync() -> None:
        descriptors = list(coordinator.descriptors.values())
        if extra is not None:
            descriptors.extend(extra(coordinator))
        added.intersection_update(d["id"] for d in descriptors)
        new: list[Entity] = []
        for desc in descriptors:
            if desc.get("platform") != platform or not desc.get("exposed", True):
                continue
            if desc["id"] in added:
                continue
            added.add(desc["id"])
            new.append(factory(coordinator, desc))
        if new:
            async_add_entities(new)

    _sync()
    entry.async_on_unload(coordinator.add_manifest_listener(_sync))
