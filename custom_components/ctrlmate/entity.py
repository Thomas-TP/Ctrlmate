"""Base entity class following Home Assistant 2026.11 Child Devices & Lifecycle APIs."""

from __future__ import annotations

from typing import Any

from homeassistant.const import EntityCategory
from homeassistant.helpers.device_registry import CONNECTION_NETWORK_MAC, DeviceInfo
from homeassistant.helpers.entity import Entity

from .const import DOMAIN
from .coordinator import CtrlmateCoordinator


class CtrlmateEntity(Entity):
    """Shared base entity for Ctrlmate with 2026.11 naming, Child Devices & in-place ID updates."""

    _attr_has_entity_name = True
    _attr_should_poll = False

    def __init__(
        self,
        coordinator: CtrlmateCoordinator,
        descriptor: dict[str, Any],
    ) -> None:
        self.coordinator = coordinator
        self.descriptor = descriptor
        self.key: str = descriptor["id"]
        self._attr_unique_id = f"{coordinator.device_id}_{self.key}"
        self._attr_name = descriptor["name"]
        if icon := descriptor.get("icon"):
            self._attr_icon = icon
        cat = descriptor.get("entity_category")
        if cat == "diagnostic":
            self._attr_entity_category = EntityCategory.DIAGNOSTIC
        elif cat == "config":
            self._attr_entity_category = EntityCategory.CONFIG
        self._unsub: Any = None

    @property
    def available(self) -> bool:
        return self.coordinator.connected

    @property
    def device_info(self) -> DeviceInfo:
        """Bind entity either to the parent PC or to a dedicated HA 2026.11 Child Device."""
        host = self.coordinator.manifest.get("host", {})
        parent_ident = (DOMAIN, self.coordinator.device_id)
        child_id = self.descriptor.get("child_device_id")

        if child_id:
            children = self.coordinator.manifest.get("child_devices", [])
            child = next((c for c in children if c.get("id") == child_id), None)
            if child:
                return DeviceInfo(
                    identifiers={(DOMAIN, f"{self.coordinator.device_id}_{child_id}")},
                    name=child.get("name", child_id),
                    manufacturer=child.get("manufacturer") or host.get("manufacturer", "Ctrlmate"),
                    model=child.get("model"),
                    hw_version=child.get("hw_version"),
                    via_device=parent_ident,
                )

        connections = set()
        if self.coordinator.mac_address:
            connections.add((CONNECTION_NETWORK_MAC, self.coordinator.mac_address))

        return DeviceInfo(
            identifiers={parent_ident},
            connections=connections,
            name=host.get("name", self.coordinator.entry.title),
            manufacturer=host.get("manufacturer", "Ctrlmate"),
            model=host.get("model", "PC"),
            sw_version=host.get("sw_version"),
        )

    def _raw_state(self) -> dict[str, Any] | None:
        return self.coordinator.states.get(self.key)

    def _raw_value(self) -> Any:
        st = self._raw_state()
        if not st:
            return None
        val = st.get("value")
        if isinstance(val, dict):
            if val.get("kind") == "unavailable":
                return None
            return val.get("value")
        return val

    @property
    def extra_state_attributes(self) -> dict[str, Any] | None:
        st = self._raw_state()
        attrs = st.get("attributes") if st else None
        return attrs if attrs else None

    async def async_added_to_hass(self) -> None:
        self._unsub = self.coordinator.add_state_listener(self.async_write_ha_state)

    async def async_will_remove_from_hass(self) -> None:
        if self._unsub:
            self._unsub()
            self._unsub = None

    async def async_entity_id_changed(self, old_entity_id: str) -> None:
        """Opt-in to Home Assistant 2026.11 in-place entity_id updates."""
        self.async_write_ha_state()
