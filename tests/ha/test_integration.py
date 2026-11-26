"""Coordinator, entities, command error mapping, Wake-on-LAN and cleanup."""

from __future__ import annotations

import asyncio
from unittest.mock import patch

import pytest
from pytest_homeassistant_custom_component.common import MockConfigEntry

from custom_components.ctrlmate.const import (
    CONF_CERT_FINGERPRINT,
    CONF_MAC_ADDRESS,
    CONF_TOKEN,
    CONF_WOL_BROADCAST,
    CONF_WOL_PORT,
    DOMAIN,
)
from homeassistant.config_entries import ConfigEntryState
from homeassistant.const import STATE_OFF, STATE_ON, STATE_UNAVAILABLE
from homeassistant.core import HomeAssistant
from homeassistant.exceptions import HomeAssistantError
from homeassistant.helpers import (
    device_registry as dr,
    entity_registry as er,
    issue_registry as ir,
)

from .conftest import make_entry
from .fake_daemon import DEVICE_ID, FakeDaemon


def _entity_id(hass: HomeAssistant, domain: str, key: str) -> str:
    entity_id = er.async_get(hass).async_get_entity_id(domain, DOMAIN, f"{DEVICE_ID}_{key}")
    assert entity_id is not None, f"no {domain} entity for {key}"
    return entity_id


def _parent(hass: HomeAssistant, entry: MockConfigEntry):
    return dr.async_get(hass).async_get_device_by_identifier((DOMAIN, DEVICE_ID), entry.entry_id)


def _child(hass: HomeAssistant, entry: MockConfigEntry, child_id: str):
    return dr.async_get(hass).async_get_child_device_by_identifier(
        (DOMAIN, f"{DEVICE_ID}_{child_id}"), entry.entry_id
    )


async def _settle(hass: HomeAssistant) -> None:
    for _ in range(5):
        await hass.async_block_till_done()
        await asyncio.sleep(0.05)


# ---------------------------------------------------------------- entities


async def test_entities_created_from_manifest(
    hass: HomeAssistant, setup_entry: MockConfigEntry
) -> None:
    ent_reg = er.async_get(hass)
    created = {
        e.unique_id for e in er.async_entries_for_config_entry(ent_reg, setup_entry.entry_id)
    }
    expected_keys = {
        "cpu_0_load", "cpu_0_temperature", "gpu_0_temperature", "cooler_0_pump_rpm",
        "active_application", "light_global", "light_cooler", "cooling_preset",
        "audio_output", "thermal_failsafe", "reboot_required", "os_updates",
        "master_volume", "audio_mute", "power_lock", "script_backup",
        "wake_on_lan", "desktop_notify", "os_pending_updates",
    }  # fmt: skip
    assert created == {f"{DEVICE_ID}_{k}" for k in expected_keys}

    # Only the zones that the manifest lists exist: no phantom lights.
    lights = [e for e in ent_reg.entities.values() if e.domain == "light"]
    assert {e.unique_id for e in lights} == {
        f"{DEVICE_ID}_light_global",
        f"{DEVICE_ID}_light_cooler",
    }

    # Translated entities have a translation_key, unknown ones keep the manifest name.
    cpu_load = ent_reg.async_get(_entity_id(hass, "sensor", "cpu_0_load"))
    assert cpu_load.translation_key == "load"
    script = ent_reg.async_get(_entity_id(hass, "button", "script_backup"))
    assert script.translation_key is None
    assert script.original_name == "Run backup"


async def test_states_and_honest_unavailable(
    hass: HomeAssistant, setup_entry: MockConfigEntry
) -> None:
    assert hass.states.get(_entity_id(hass, "sensor", "cpu_0_load")).state == "12.5"
    assert hass.states.get(_entity_id(hass, "sensor", "gpu_0_temperature")).state == "51.0"
    # A null sensor is unavailable, never 0.
    temp = hass.states.get(_entity_id(hass, "sensor", "cpu_0_temperature"))
    assert temp.state == STATE_UNAVAILABLE
    assert hass.states.get(_entity_id(hass, "binary_sensor", "reboot_required")).state == STATE_ON
    assert hass.states.get(_entity_id(hass, "binary_sensor", "thermal_failsafe")).state == STATE_OFF
    assert hass.states.get(_entity_id(hass, "number", "master_volume")).state == "35.0"
    assert hass.states.get(_entity_id(hass, "switch", "audio_mute")).state == STATE_OFF


async def test_light_attributes(hass: HomeAssistant, setup_entry: MockConfigEntry) -> None:
    master = hass.states.get(_entity_id(hass, "light", "light_global"))
    assert master.state == STATE_ON
    assert master.attributes["brightness"] == 204
    assert master.attributes["color_temp_kelvin"] == 4000
    assert master.attributes["min_color_temp_kelvin"] == 2000
    assert master.attributes["max_color_temp_kelvin"] == 6500
    assert set(master.attributes["supported_color_modes"]) == {"rgb", "color_temp"}
    assert "effect_list" not in master.attributes
    zone = hass.states.get(_entity_id(hass, "light", "light_cooler"))
    assert zone.state == STATE_OFF


async def test_cooling_select_has_five_presets(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    state = hass.states.get(_entity_id(hass, "select", "cooling_preset"))
    assert state.state == "balanced"
    assert state.attributes["options"] == ["auto", "silent", "balanced", "performance", "gaming"]

    await hass.services.async_call(
        "select",
        "select_option",
        {"entity_id": state.entity_id, "option": "gaming"},
        blocking=True,
    )
    assert daemon.commands[-1] == {
        "action": "set_cooling_preset",
        "entity_id": "cooling_preset",
        "preset": "gaming",
    }


async def test_update_entity_has_no_fake_version(
    hass: HomeAssistant, setup_entry: MockConfigEntry
) -> None:
    update = hass.states.get(_entity_id(hass, "update", "os_updates"))
    assert update.state == STATE_ON
    assert update.attributes["installed_version"] == "10.0.26100"
    assert update.attributes["latest_version"] == "KB5099999, KB2267602"
    assert update.attributes["kb_ids"] == ["KB5099999", "KB2267602"]
    assert update.attributes["pending_titles"] == [
        "2026-11 Cumulative Update",
        "Defender definitions",
    ]
    assert len(update.attributes["release_summary"]) <= 255
    assert update.attributes["supported_features"] & 4  # PROGRESS
    assert update.attributes["supported_features"] & 1  # INSTALL
    pending = hass.states.get(_entity_id(hass, "sensor", "os_pending_updates"))
    assert pending.state == "2"


async def test_update_unknown_on_error_and_no_pending(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    entity_id = _entity_id(hass, "update", "os_updates")
    base = {"installed_version": "10.0.26100", "pending_count": 0, "pending_titles": []}
    await daemon.broadcast(
        {
            "type": "state_batch",
            "states": [
                {"id": "os_updates", "value": {"kind": "update", "value": base}},
            ],
        }
    )
    await _settle(hass)
    assert hass.states.get(entity_id).state == STATE_OFF

    await daemon.broadcast(
        {
            "type": "state_batch",
            "states": [
                {
                    "id": "os_updates",
                    "value": {"kind": "update", "value": {**base, "error": "wua_unavailable"}},
                }
            ],
        }
    )
    await _settle(hass)
    assert hass.states.get(entity_id).state == "unknown"
    assert hass.states.get(entity_id).attributes["error"] == "wua_unavailable"
    assert (
        hass.states.get(_entity_id(hass, "sensor", "os_pending_updates")).state == STATE_UNAVAILABLE
    )


async def test_state_push_updates_entity(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    await daemon.broadcast(
        {
            "type": "state_batch",
            "states": [{"id": "cpu_0_load", "value": {"kind": "float", "value": 99.0}}],
        }
    )
    await _settle(hass)
    assert hass.states.get(_entity_id(hass, "sensor", "cpu_0_load")).state == "99.0"


async def test_children_inherit_parent_area(
    hass: HomeAssistant, setup_entry: MockConfigEntry
) -> None:
    parent = _parent(hass, setup_entry)
    child = _child(hass, setup_entry, "gpu_0")
    assert parent is not None and child is not None
    assert child.parent_device_id == parent.id  # documented ChildDeviceInfo pattern
    dev_reg = dr.async_get(hass)
    dev_reg.async_update_device(parent.id, area_id="office")
    assert dr.async_get_effective_area_id(hass, dev_reg.async_get(child.id)) == "office"


# ---------------------------------------------------------------- commands


async def test_command_success_sends_wire_format(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    await hass.services.async_call(
        "light",
        "turn_on",
        {
            "entity_id": _entity_id(hass, "light", "light_global"),
            "brightness": 120,
            "color_temp_kelvin": 2700,
            "transition": 1.5,
        },
        blocking=True,
    )
    assert daemon.commands[-1] == {
        "action": "set_light",
        "entity_id": "light_global",
        "is_on": True,
        "brightness": 120,
        "color_temp_kelvin": 2700,
        "transition_ms": 1500,
    }
    await hass.services.async_call(
        "number",
        "set_value",
        {"entity_id": _entity_id(hass, "number", "master_volume"), "value": 40},
        blocking=True,
    )
    assert daemon.commands[-1] == {
        "action": "set_number",
        "entity_id": "master_volume",
        "value": 40.0,
    }


@pytest.mark.parametrize(
    ("code", "message_part"),
    [
        ("permission_denied", "disabled in the Ctrlmate permissions"),
        ("no_agent", "agent is not running"),
        ("agent_timeout", "did not answer in time"),
        ("not_supported", "does not support"),
        ("invalid_argument", "rejected the value: bad"),
        ("hardware_error", "reported an error: bad"),
        ("busy", "busy"),
        ("unavailable", "currently unavailable"),
        ("internal", "internal error: bad"),
        ("some_future_code", "internal error: bad"),
    ],
)
async def test_command_error_mapping(
    hass: HomeAssistant,
    daemon: FakeDaemon,
    setup_entry: MockConfigEntry,
    code: str,
    message_part: str,
) -> None:
    daemon.default_result = {"ok": False, "code": code, "message": "bad"}
    with pytest.raises(HomeAssistantError) as exc:
        await hass.services.async_call(
            "button",
            "press",
            {"entity_id": _entity_id(hass, "button", "power_lock")},
            blocking=True,
        )
    expected = code if code != "some_future_code" else "internal"
    assert exc.value.translation_domain == DOMAIN
    assert exc.value.translation_key == f"command_{expected}"
    assert exc.value.translation_placeholders["name"] == "Gaming PC"
    # The message resolves through strings.json (the English translation).
    from homeassistant.helpers.translation import async_get_exception_message

    assert message_part in async_get_exception_message(
        DOMAIN, exc.value.translation_key, exc.value.translation_placeholders
    )


async def test_old_daemon_without_code_maps_to_internal(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    daemon.default_result = {"ok": False, "message": "legacy failure"}
    with pytest.raises(HomeAssistantError) as exc:
        await hass.services.async_call(
            "switch",
            "turn_on",
            {"entity_id": _entity_id(hass, "switch", "audio_mute")},
            blocking=True,
        )
    assert exc.value.translation_key == "command_internal"


async def test_no_agent_raises_and_clears_repair_issue(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    issue_reg = ir.async_get(hass)
    issue_id = f"no_agent_{DEVICE_ID}"
    daemon.default_result = {"ok": False, "code": "no_agent"}
    with pytest.raises(HomeAssistantError):
        await hass.services.async_call(
            "button",
            "press",
            {"entity_id": _entity_id(hass, "button", "power_lock")},
            blocking=True,
        )
    assert issue_reg.async_get_issue(DOMAIN, issue_id) is not None

    daemon.default_result = {"ok": True, "code": "ok"}
    await hass.services.async_call(
        "button", "press", {"entity_id": _entity_id(hass, "button", "power_lock")}, blocking=True
    )
    assert issue_reg.async_get_issue(DOMAIN, issue_id) is None


async def test_connection_dropped_mid_command_is_wrapped(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    daemon.drop_on_command = True
    with pytest.raises(HomeAssistantError) as exc:
        await hass.services.async_call(
            "button",
            "press",
            {"entity_id": _entity_id(hass, "button", "power_lock")},
            blocking=True,
        )
    assert exc.value.translation_key == "connection_lost"
    coordinator = setup_entry.runtime_data
    assert coordinator._pending == {}


async def test_command_timeout(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    daemon.default_result = {"ok": True, "code": "ok"}
    with patch("custom_components.ctrlmate.coordinator.COMMAND_TIMEOUT", 0.2):
        # The daemon never answers this one.
        daemon.command_results["press_button"] = {}
        original = daemon.sockets[0].send_json

        async def swallow(data, *args, **kwargs):
            if data.get("type") == "command_result":
                return None
            return await original(data, *args, **kwargs)

        daemon.sockets[0].send_json = swallow
        with pytest.raises(HomeAssistantError) as exc:
            await hass.services.async_call(
                "button",
                "press",
                {"entity_id": _entity_id(hass, "button", "power_lock")},
                blocking=True,
            )
    assert exc.value.translation_key == "command_timeout"


async def test_offline_command_is_translated_error(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    coordinator = setup_entry.runtime_data
    await coordinator.async_stop()
    with pytest.raises(HomeAssistantError) as exc:
        await coordinator.async_send_command({"action": "check_os_updates"})
    assert exc.value.translation_key == "pc_offline"


# ----------------------------------------------------------- connection


async def test_revoked_token_starts_reauth(hass: HomeAssistant, daemon: FakeDaemon) -> None:
    entry = make_entry(daemon, **{CONF_TOKEN: "revoked"})
    entry.add_to_hass(hass)
    assert not await hass.config_entries.async_setup(entry.entry_id)
    await hass.async_block_till_done()
    assert entry.state is ConfigEntryState.SETUP_ERROR
    flows = hass.config_entries.flow.async_progress_by_handler(DOMAIN)
    assert any(f["context"]["source"] == "reauth" for f in flows)


async def test_wrong_pinned_fingerprint_fails_closed(
    hass: HomeAssistant, daemon: FakeDaemon
) -> None:
    entry = make_entry(daemon, **{CONF_CERT_FINGERPRINT: "00" * 32})
    entry.add_to_hass(hass)
    assert not await hass.config_entries.async_setup(entry.entry_id)
    await hass.async_block_till_done()
    assert entry.state is ConfigEntryState.SETUP_ERROR
    assert daemon.commands == []
    assert daemon.sockets == []  # no authenticated session was ever opened
    flows = hass.config_entries.flow.async_progress_by_handler(DOMAIN)
    assert any(f["context"]["source"] == "reauth" for f in flows)


async def test_offline_pc_sets_up_and_wol_still_works(
    hass: HomeAssistant, daemon: FakeDaemon
) -> None:
    port = daemon.port
    await daemon.stop()
    entry = make_entry(daemon, port=port)
    entry.add_to_hass(hass)
    assert await hass.config_entries.async_setup(entry.entry_id)
    await hass.async_block_till_done()
    assert entry.state is ConfigEntryState.LOADED
    assert not entry.runtime_data.connected
    # Wake-on-LAN does not depend on the connection.
    wake = hass.states.get(_entity_id(hass, "button", "wake_on_lan"))
    assert wake.state != STATE_UNAVAILABLE
    with patch("custom_components.ctrlmate.coordinator.send_wol_magic_packet") as send:
        await hass.services.async_call(
            "button", "press", {"entity_id": wake.entity_id}, blocking=True
        )
    send.assert_called_once()
    await hass.config_entries.async_unload(entry.entry_id)


# ----------------------------------------------------------- Wake-on-LAN


async def test_wake_on_lan_uses_options(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    hass.config_entries.async_update_entry(
        setup_entry, options={CONF_WOL_BROADCAST: "192.168.7.255", CONF_WOL_PORT: 7}
    )
    with patch("custom_components.ctrlmate.coordinator.send_wol_magic_packet") as send:
        await hass.services.async_call(
            "button",
            "press",
            {"entity_id": _entity_id(hass, "button", "wake_on_lan")},
            blocking=True,
        )
    send.assert_called_once_with("aa:bb:cc:dd:ee:ff", "192.168.7.255", 7)


async def test_wake_on_lan_default_is_subnet_broadcast(hass: HomeAssistant) -> None:
    from custom_components.ctrlmate.coordinator import async_default_broadcast

    adapters = [{"ipv4": [{"address": "192.168.1.10", "network_prefix": 24}]}]
    with patch(
        "custom_components.ctrlmate.coordinator.network.async_get_adapters",
        return_value=adapters,
    ):
        assert await async_default_broadcast(hass, "192.168.1.50") == "192.168.1.255"
        # Not on a local subnet, hostname or IPv6: limited broadcast fallback.
        assert await async_default_broadcast(hass, "10.0.0.5") == "255.255.255.255"
        assert await async_default_broadcast(hass, "pc.local") == "255.255.255.255"
        assert await async_default_broadcast(hass, "fd00::1") == "255.255.255.255"


async def test_wake_on_lan_without_option_uses_default_port(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    with (
        patch(
            "custom_components.ctrlmate.coordinator.async_default_broadcast",
            return_value="192.168.1.255",
        ),
        patch("custom_components.ctrlmate.coordinator.send_wol_magic_packet") as send,
    ):
        await setup_entry.runtime_data.async_wake_pc()
    send.assert_called_once_with("aa:bb:cc:dd:ee:ff", "192.168.1.255", 9)


async def test_manifest_updates_stored_mac(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    manifest = daemon.manifest_copy()
    manifest["host"]["mac_address"] = "11:22:33:44:55:66"
    await daemon.broadcast(manifest)
    await _settle(hass)
    assert setup_entry.data[CONF_MAC_ADDRESS] == "11:22:33:44:55:66"


# ----------------------------------------------------------- cleanup


async def test_stale_child_and_entities_are_removed(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    assert _child(hass, setup_entry, "cooler_0") is not None

    manifest = daemon.manifest_copy()
    manifest["child_devices"] = [c for c in manifest["child_devices"] if c["id"] != "cooler_0"]
    manifest["entities"] = [
        e for e in manifest["entities"] if e.get("child_device_id") != "cooler_0"
    ]
    await daemon.broadcast(manifest)
    await _settle(hass)

    assert _child(hass, setup_entry, "cooler_0") is None
    assert _child(hass, setup_entry, "gpu_0") is not None
    ent_reg = er.async_get(hass)
    assert ent_reg.async_get_entity_id("light", DOMAIN, f"{DEVICE_ID}_light_cooler") is None
    # Static and still-listed entities stay.
    assert ent_reg.async_get_entity_id("button", DOMAIN, f"{DEVICE_ID}_wake_on_lan") is not None
    assert ent_reg.async_get_entity_id("light", DOMAIN, f"{DEVICE_ID}_light_global") is not None


async def test_remove_config_entry_device_guard(
    hass: HomeAssistant, setup_entry: MockConfigEntry
) -> None:
    from custom_components.ctrlmate import async_remove_config_entry_device

    dev_reg = dr.async_get(hass)
    parent = _parent(hass, setup_entry)
    live_child = _child(hass, setup_entry, "gpu_0")
    stale = dev_reg.async_get_or_create_child(
        config_entry_id=setup_entry.entry_id,
        identifiers={(DOMAIN, f"{DEVICE_ID}_gone")},
        parent_device_id=parent.id,
        name="Gone",
    )
    assert not await async_remove_config_entry_device(hass, setup_entry, parent)
    assert not await async_remove_config_entry_device(hass, setup_entry, live_child)
    assert await async_remove_config_entry_device(hass, setup_entry, stale)


async def test_old_service_creates_repair_issue(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    manifest = daemon.manifest_copy()
    manifest["host"]["sw_version"] = "0.5.0"
    await daemon.broadcast(manifest)
    await _settle(hass)
    issue = ir.async_get(hass).async_get_issue(DOMAIN, f"service_outdated_{DEVICE_ID}")
    assert issue is not None
    assert issue.translation_placeholders["required"] == "0.6.0"


async def test_diagnostics_are_redacted(
    hass: HomeAssistant, daemon: FakeDaemon, setup_entry: MockConfigEntry
) -> None:
    from custom_components.ctrlmate.diagnostics import async_get_config_entry_diagnostics

    diag = await async_get_config_entry_diagnostics(hass, setup_entry)
    dumped = str(diag)
    assert daemon.fingerprint not in dumped
    assert "aa:bb:cc:dd:ee:ff" not in dumped.lower()
    assert "AA:BB:CC:DD:EE:FF" not in dumped
    assert "127.0.0.1" not in dumped
    assert setup_entry.data[CONF_TOKEN] not in dumped
    assert diag["connected"] is True
    assert any(e["id"] == "light_global" for e in diag["entities"])
