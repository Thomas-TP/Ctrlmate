"""Update platform: pending operating system updates (Windows Update / PackageKit)."""

from __future__ import annotations

from typing import Any

from homeassistant.components.update import UpdateEntity, UpdateEntityFeature
from homeassistant.core import HomeAssistant
from homeassistant.helpers.entity_platform import AddEntitiesCallback

from . import CtrlmateConfigEntry
from .entity import (
    CtrlmateEntity,
    async_setup_manifest_platform,
    update_kb_ids,
    update_pending_count,
    update_titles,
)

PARALLEL_UPDATES = 1

MAX_SUMMARY = 255


async def async_setup_entry(
    hass: HomeAssistant,
    entry: CtrlmateConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    async_setup_manifest_platform(hass, entry, async_add_entities, "update", CtrlmateUpdateEntity)


def _clip(text: str, limit: int = MAX_SUMMARY) -> str:
    return text if len(text) <= limit else text[: limit - 1] + "…"


class CtrlmateUpdateEntity(CtrlmateEntity, UpdateEntity):
    """OS updates without invented version numbers.

    ``installed_version`` is the real OS version. ``latest_version`` equals it when
    nothing is pending, otherwise it lists the pending KB ids (or a pending count
    when the platform has no KB ids). The count has its own sensor.
    """

    _attr_supported_features = UpdateEntityFeature.INSTALL | UpdateEntityFeature.PROGRESS

    def _info(self) -> dict[str, Any]:
        value = self._raw_value()
        return value if isinstance(value, dict) else {}

    @property
    def installed_version(self) -> str | None:
        version = self._info().get("installed_version")
        if not version:
            version = self.coordinator.manifest.get("host", {}).get("os_version")
        return str(version) if version else None

    @property
    def latest_version(self) -> str | None:
        info = self._info()
        installed = self.installed_version
        pending = update_pending_count(info)
        if pending is None or installed is None:
            return None  # unknown rather than a made-up answer
        if pending <= 0:
            return installed
        kb_ids = update_kb_ids(info)
        return _clip(", ".join(kb_ids)) if kb_ids else f"{pending} pending"

    @property
    def in_progress(self) -> bool:
        return bool(self._info().get("in_progress", False))

    @property
    def release_summary(self) -> str | None:
        titles = update_titles(self._info())
        return _clip("; ".join(titles)) if titles else None

    @property
    def extra_state_attributes(self) -> dict[str, Any] | None:
        info = self._info()
        if not info:
            return None
        attrs: dict[str, Any] = {
            "pending_count": update_pending_count(info),
            "pending_titles": update_titles(info),
            "kb_ids": update_kb_ids(info),
            "reboot_required": bool(info.get("reboot_required", False)),
            "last_checked": info.get("last_checked"),
        }
        if info.get("error"):
            attrs["error"] = info["error"]
        return attrs

    async def async_install(self, version: str | None, backup: bool, **kwargs: Any) -> None:
        await self.coordinator.async_send_command({"action": "install_os_updates"})
