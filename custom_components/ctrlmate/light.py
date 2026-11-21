"""Light platform for Ctrlmate (Brightness + RGB + Kelvin 2000K-6500K for Hue & Govee sync)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.light import (
    ATTR_BRIGHTNESS,
    ATTR_COLOR_TEMP_KELVIN,
    ATTR_RGB_COLOR,
    ATTR_TRANSITION,
    ColorMode,
    LightEntity,
    LightEntityFeature,
)
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
            if desc.get("platform") != "light" or not desc.get("exposed", True):
                continue
            eid = desc["id"]
            if eid not in added:
                added.add(eid)
                new_entities.append(CtrlmateLight(coordinator, desc))
        if new_entities:
            async_add_entities(new_entities)

    _sync_entities()
    entry.async_on_unload(coordinator.add_manifest_listener(_sync_entities))


class CtrlmateLight(CtrlmateEntity, LightEntity):
    """PC lighting zone supporting Brightness, RGB and Kelvin color temperature."""

    _attr_supported_color_modes = {ColorMode.RGB, ColorMode.COLOR_TEMP}
    _attr_supported_features = LightEntityFeature.TRANSITION

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        caps = descriptor.get("light_caps") or {}
        self._attr_min_color_temp_kelvin = int(caps.get("min_color_temp_kelvin", 2000))
        self._attr_max_color_temp_kelvin = int(caps.get("max_color_temp_kelvin", 6500))

    def _light_state(self) -> dict[str, Any]:
        val = self._raw_value()
        return val if isinstance(val, dict) else {}

    @property
    def is_on(self) -> bool | None:
        st = self._light_state()
        return bool(st["is_on"]) if "is_on" in st else None

    @property
    def brightness(self) -> int | None:
        st = self._light_state()
        return int(st["brightness"]) if "brightness" in st else None

    @property
    def color_mode(self) -> ColorMode:
        st = self._light_state()
        if st.get("color_mode") == "color_temp":
            return ColorMode.COLOR_TEMP
        return ColorMode.RGB

    @property
    def rgb_color(self) -> tuple[int, int, int] | None:
        rgb = self._light_state().get("rgb")
        if isinstance(rgb, dict):
            return (int(rgb["r"]), int(rgb["g"]), int(rgb["b"]))
        return None

    @property
    def color_temp_kelvin(self) -> int | None:
        st = self._light_state()
        return int(st["color_temp_kelvin"]) if "color_temp_kelvin" in st else None

    async def async_turn_on(self, **kwargs: Any) -> None:
        cmd: dict[str, Any] = {
            "action": "set_light",
            "entity_id": self.key,
            "is_on": True,
        }
        if ATTR_BRIGHTNESS in kwargs:
            cmd["brightness"] = int(kwargs[ATTR_BRIGHTNESS])
        if ATTR_COLOR_TEMP_KELVIN in kwargs:
            cmd["color_temp_kelvin"] = int(kwargs[ATTR_COLOR_TEMP_KELVIN])
        elif ATTR_RGB_COLOR in kwargs:
            r, g, b = kwargs[ATTR_RGB_COLOR]
            cmd["rgb"] = {"r": int(r), "g": int(g), "b": int(b)}
        if ATTR_TRANSITION in kwargs:
            cmd["transition_ms"] = int(float(kwargs[ATTR_TRANSITION]) * 1000)

        await self.coordinator.async_send_command(cmd)

    async def async_turn_off(self, **kwargs: Any) -> None:
        cmd: dict[str, Any] = {
            "action": "set_light",
            "entity_id": self.key,
            "is_on": False,
        }
        if ATTR_TRANSITION in kwargs:
            cmd["transition_ms"] = int(float(kwargs[ATTR_TRANSITION]) * 1000)
        await self.coordinator.async_send_command(cmd)
