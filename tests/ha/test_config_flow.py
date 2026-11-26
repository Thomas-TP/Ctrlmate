"""Config flow: TOFU pinning, pairing, duplicates, reauth, reconfigure, options."""

from __future__ import annotations

from custom_components.ctrlmate.const import (
    CONF_CERT_FINGERPRINT,
    CONF_CONFIRM,
    CONF_PIN,
    CONF_TOKEN,
    CONF_WOL_BROADCAST,
    CONF_WOL_PORT,
    DOMAIN,
)
from custom_components.ctrlmate.tls import format_fingerprint
from homeassistant import config_entries
from homeassistant.const import CONF_HOST, CONF_PORT
from homeassistant.core import HomeAssistant
from homeassistant.data_entry_flow import FlowResultType
from homeassistant.helpers.service_info.zeroconf import ZeroconfServiceInfo

from .conftest import make_entry
from .fake_daemon import DEVICE_ID, PIN, FakeDaemon


async def _settle(hass: HomeAssistant, entry) -> None:
    """Let the reload scheduled by the flow finish, then stop the connection."""
    await hass.async_block_till_done()
    await hass.config_entries.async_unload(entry.entry_id)
    await hass.async_block_till_done()


async def _start_user(hass: HomeAssistant, daemon: FakeDaemon):
    result = await hass.config_entries.flow.async_init(
        DOMAIN, context={"source": config_entries.SOURCE_USER}
    )
    assert result["type"] is FlowResultType.FORM
    assert result["step_id"] == "user"
    return await hass.config_entries.flow.async_configure(
        result["flow_id"], {CONF_HOST: "127.0.0.1", CONF_PORT: daemon.port}
    )


def _zeroconf(daemon: FakeDaemon, fp: str | None, host: str = "127.0.0.1"):
    props = {"id": DEVICE_ID, "name": "Gaming PC", "ver": "0.6.0"}
    if fp is not None:
        props["fp"] = fp
    return ZeroconfServiceInfo(
        ip_address=__import__("ipaddress").ip_address(host),
        ip_addresses=[__import__("ipaddress").ip_address(host)],
        port=daemon.port,
        hostname="gaming-pc.local.",
        type="_ctrlmate._tcp.local.",
        name="Gaming PC._ctrlmate._tcp.local.",
        properties=props,
    )


async def test_user_flow_requires_fingerprint_confirmation(
    hass: HomeAssistant, daemon: FakeDaemon
) -> None:
    result = await _start_user(hass, daemon)
    assert result["step_id"] == "confirm_fingerprint"
    assert result["description_placeholders"]["fingerprint"] == format_fingerprint(
        daemon.fingerprint
    )

    # Not ticked -> stay on the step with an error, nothing is paired.
    result = await hass.config_entries.flow.async_configure(
        result["flow_id"], {CONF_CONFIRM: False}
    )
    assert result["errors"] == {"base": "fingerprint_not_confirmed"}
    assert daemon.pair_attempts == 0

    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_CONFIRM: True})
    assert result["step_id"] == "pair"

    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_PIN: PIN})
    assert result["type"] is FlowResultType.CREATE_ENTRY
    assert result["title"] == "Gaming PC"
    assert result["data"][CONF_CERT_FINGERPRINT] == daemon.fingerprint
    assert result["data"][CONF_TOKEN]
    assert result["result"].unique_id == DEVICE_ID


async def test_wrong_pin_then_right_pin(hass: HomeAssistant, daemon: FakeDaemon) -> None:
    result = await _start_user(hass, daemon)
    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_CONFIRM: True})
    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_PIN: "000000"})
    assert result["type"] is FlowResultType.FORM
    assert result["errors"] == {"base": "invalid_pin"}

    # Malformed PINs are rejected locally, without a request.
    attempts = daemon.pair_attempts
    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_PIN: "12ab"})
    assert result["errors"] == {"base": "invalid_pin"}
    assert daemon.pair_attempts == attempts

    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_PIN: PIN})
    assert result["type"] is FlowResultType.CREATE_ENTRY


async def test_unreachable_host(hass: HomeAssistant, daemon: FakeDaemon) -> None:
    result = await hass.config_entries.flow.async_init(
        DOMAIN, context={"source": config_entries.SOURCE_USER}
    )
    result = await hass.config_entries.flow.async_configure(
        result["flow_id"], {CONF_HOST: "127.0.0.1", CONF_PORT: 1}
    )
    assert result["type"] is FlowResultType.FORM
    assert result["errors"] == {"base": "cannot_connect"}


async def test_zeroconf_matching_fingerprint_skips_confirmation(
    hass: HomeAssistant, daemon: FakeDaemon
) -> None:
    result = await hass.config_entries.flow.async_init(
        DOMAIN,
        context={"source": config_entries.SOURCE_ZEROCONF},
        data=_zeroconf(daemon, daemon.fingerprint.upper()),
    )
    assert result["type"] is FlowResultType.FORM
    assert result["step_id"] == "pair"
    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_PIN: PIN})
    assert result["type"] is FlowResultType.CREATE_ENTRY
    assert result["data"][CONF_CERT_FINGERPRINT] == daemon.fingerprint


async def test_zeroconf_wrong_fingerprint_aborts(hass: HomeAssistant, daemon: FakeDaemon) -> None:
    result = await hass.config_entries.flow.async_init(
        DOMAIN,
        context={"source": config_entries.SOURCE_ZEROCONF},
        data=_zeroconf(daemon, "ab" * 32),
    )
    assert result["type"] is FlowResultType.ABORT
    assert result["reason"] == "fingerprint_mismatch"
    assert daemon.pair_attempts == 0


async def test_zeroconf_without_fingerprint_asks_user(
    hass: HomeAssistant, daemon: FakeDaemon
) -> None:
    result = await hass.config_entries.flow.async_init(
        DOMAIN,
        context={"source": config_entries.SOURCE_ZEROCONF},
        data=_zeroconf(daemon, None),
    )
    assert result["step_id"] == "confirm_fingerprint"


async def test_user_duplicate_aborts_before_pairing(
    hass: HomeAssistant, daemon: FakeDaemon
) -> None:
    make_entry(daemon).add_to_hass(hass)
    result = await _start_user(hass, daemon)
    assert result["type"] is FlowResultType.ABORT
    assert result["reason"] == "already_configured"
    assert daemon.pair_attempts == 0


async def test_zeroconf_updates_host_of_existing_entry(
    hass: HomeAssistant, daemon: FakeDaemon
) -> None:
    entry = make_entry(daemon, host="10.9.9.9")
    entry.add_to_hass(hass)
    result = await hass.config_entries.flow.async_init(
        DOMAIN,
        context={"source": config_entries.SOURCE_ZEROCONF},
        data=_zeroconf(daemon, daemon.fingerprint),
    )
    assert result["type"] is FlowResultType.ABORT
    assert result["reason"] == "already_configured"
    assert entry.data[CONF_HOST] == "127.0.0.1"


async def test_zeroconf_with_other_fingerprint_does_not_hijack_entry(
    hass: HomeAssistant, daemon: FakeDaemon
) -> None:
    entry = make_entry(daemon, host="10.9.9.9")
    entry.add_to_hass(hass)
    result = await hass.config_entries.flow.async_init(
        DOMAIN,
        context={"source": config_entries.SOURCE_ZEROCONF},
        data=_zeroconf(daemon, "cd" * 32),
    )
    assert result["reason"] == "already_configured"
    assert entry.data[CONF_HOST] == "10.9.9.9"


async def test_reauth_flow_updates_token(hass: HomeAssistant, daemon: FakeDaemon) -> None:
    entry = make_entry(daemon, **{CONF_TOKEN: "revoked-token"})
    entry.add_to_hass(hass)

    result = await entry.start_reauth_flow(hass)
    assert result["step_id"] == "reauth_confirm"
    result = await hass.config_entries.flow.async_configure(result["flow_id"], {})
    # Certificate unchanged: still pinned, go straight to the PIN.
    assert result["step_id"] == "pair"
    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_PIN: PIN})
    assert result["type"] is FlowResultType.ABORT
    assert result["reason"] == "reauth_successful"
    assert entry.data[CONF_TOKEN] != "revoked-token"
    await _settle(hass, entry)


async def test_reauth_with_changed_certificate_asks_again(
    hass: HomeAssistant, daemon: FakeDaemon
) -> None:
    entry = make_entry(daemon, **{CONF_CERT_FINGERPRINT: "ef" * 32})
    entry.add_to_hass(hass)
    result = await entry.start_reauth_flow(hass)
    result = await hass.config_entries.flow.async_configure(result["flow_id"], {})
    assert result["step_id"] == "confirm_fingerprint"
    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_CONFIRM: True})
    result = await hass.config_entries.flow.async_configure(result["flow_id"], {CONF_PIN: PIN})
    assert result["reason"] == "reauth_successful"
    assert entry.data[CONF_CERT_FINGERPRINT] == daemon.fingerprint
    await _settle(hass, entry)


async def test_reconfigure_updates_address(hass: HomeAssistant, daemon: FakeDaemon) -> None:
    entry = make_entry(daemon, **{CONF_PORT: 1})
    entry.add_to_hass(hass)
    result = await entry.start_reconfigure_flow(hass)
    assert result["step_id"] == "reconfigure"
    result = await hass.config_entries.flow.async_configure(
        result["flow_id"], {CONF_HOST: "127.0.0.1", CONF_PORT: daemon.port}
    )
    assert result["type"] is FlowResultType.ABORT
    assert result["reason"] == "reconfigure_successful"
    assert entry.data[CONF_PORT] == daemon.port
    await _settle(hass, entry)


async def test_options_flow_validates_broadcast(hass: HomeAssistant, daemon: FakeDaemon) -> None:
    entry = make_entry(daemon)
    entry.add_to_hass(hass)
    result = await hass.config_entries.options.async_init(entry.entry_id)
    result = await hass.config_entries.options.async_configure(
        result["flow_id"], {CONF_WOL_BROADCAST: "not-an-ip", CONF_WOL_PORT: 9}
    )
    assert result["errors"] == {CONF_WOL_BROADCAST: "invalid_broadcast"}
    result = await hass.config_entries.options.async_configure(
        result["flow_id"], {CONF_WOL_BROADCAST: "192.168.1.255", CONF_WOL_PORT: 7}
    )
    assert result["type"] is FlowResultType.CREATE_ENTRY
    assert entry.options == {CONF_WOL_BROADCAST: "192.168.1.255", CONF_WOL_PORT: 7}
