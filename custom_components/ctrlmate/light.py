"""Light platform: the master light and every real lighting zone of the manifest."""

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
from .entity import CtrlmateEntity, async_setup_manifest_platform

PARALLEL_UPDATES = 1


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    async_setup_manifest_platform(hass, entry, async_add_entities, "light", CtrlmateLight)


class CtrlmateLight(CtrlmateEntity, LightEntity):
    """RGB / Kelvin / brightness zone (no effects)."""

    _attr_supported_features = LightEntityFeature.TRANSITION

    def __init__(self, coordinator: CtrlmateCoordinator, descriptor: dict[str, Any]) -> None:
        super().__init__(coordinator, descriptor)
        caps = descriptor.get("light_caps") or {}
        modes: set[ColorMode] = set()
        if caps.get("supports_color_temp", True):
            modes.add(ColorMode.COLOR_TEMP)
        if caps.get("supports_rgb", True):
            modes.add(ColorMode.RGB)
        self._attr_supported_color_modes = modes or {ColorMode.BRIGHTNESS}
        if not caps.get("supports_transition", True):
            self._attr_supported_features = LightEntityFeature(0)
        self._attr_min_color_temp_kelvin = int(caps.get("min_color_temp_kelvin", 2000))
        self._attr_max_color_temp_kelvin = int(caps.get("max_color_temp_kelvin", 6500))

    def _light(self) -> dict[str, Any]:
        value = self._raw_value()
        return value if isinstance(value, dict) else {}

    @property
    def is_on(self) -> bool | None:
        state = self._light()
        return bool(state["is_on"]) if "is_on" in state else None

    @property
    def brightness(self) -> int | None:
        state = self._light()
        return int(state["brightness"]) if state.get("brightness") is not None else None

    @property
    def color_mode(self) -> ColorMode:
        supported = self._attr_supported_color_modes or {ColorMode.BRIGHTNESS}
        wanted = (
            ColorMode.COLOR_TEMP
            if self._light().get("color_mode") == "color_temp"
            else ColorMode.RGB
        )
        if wanted in supported:
            return wanted
        return next(iter(supported))

    @property
    def rgb_color(self) -> tuple[int, int, int] | None:
        rgb = self._light().get("rgb")
        if isinstance(rgb, dict):
            return (int(rgb["r"]), int(rgb["g"]), int(rgb["b"]))
        return None

    @property
    def color_temp_kelvin(self) -> int | None:
        value = self._light().get("color_temp_kelvin")
        return int(value) if value is not None else None

    async def async_turn_on(self, **kwargs: Any) -> None:
        command: dict[str, Any] = {
            "action": "set_light",
            "entity_id": self.key,
            "is_on": True,
        }
        if ATTR_BRIGHTNESS in kwargs:
            command["brightness"] = int(kwargs[ATTR_BRIGHTNESS])
        if ATTR_COLOR_TEMP_KELVIN in kwargs:
            command["color_temp_kelvin"] = int(kwargs[ATTR_COLOR_TEMP_KELVIN])
        elif ATTR_RGB_COLOR in kwargs:
            r, g, b = kwargs[ATTR_RGB_COLOR]
            command["rgb"] = {"r": int(r), "g": int(g), "b": int(b)}
        if ATTR_TRANSITION in kwargs:
            command["transition_ms"] = int(float(kwargs[ATTR_TRANSITION]) * 1000)
        await self.coordinator.async_send_command(command)

    async def async_turn_off(self, **kwargs: Any) -> None:
        command: dict[str, Any] = {
            "action": "set_light",
            "entity_id": self.key,
            "is_on": False,
        }
        if ATTR_TRANSITION in kwargs:
            command["transition_ms"] = int(float(kwargs[ATTR_TRANSITION]) * 1000)
        await self.coordinator.async_send_command(command)
