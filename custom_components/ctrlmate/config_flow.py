"""Config flow: discovery, trust-on-first-use pinning, PIN pairing, reauth, reconfigure."""

from __future__ import annotations

from collections.abc import Mapping
import ipaddress
import logging
from typing import Any

import aiohttp
import voluptuous as vol

from homeassistant.config_entries import (
    ConfigEntry,
    ConfigFlow,
    ConfigFlowResult,
    OptionsFlow,
)
from homeassistant.const import CONF_HOST, CONF_PORT
from homeassistant.core import callback
from homeassistant.helpers.aiohttp_client import async_get_clientsession
from homeassistant.helpers.service_info.zeroconf import ZeroconfServiceInfo

from .const import (
    CONF_CERT_FINGERPRINT,
    CONF_CLIENT_ID,
    CONF_CONFIRM,
    CONF_DEVICE_ID,
    CONF_MAC_ADDRESS,
    CONF_PIN,
    CONF_TOKEN,
    CONF_WOL_BROADCAST,
    CONF_WOL_PORT,
    DEFAULT_PORT,
    DEFAULT_WOL_PORT,
    DOMAIN,
)
from .tls import (
    CtrlmateTlsError,
    async_get_peer_fingerprint,
    format_fingerprint,
    format_host,
    normalize_fingerprint,
    pinned_ssl,
)

_LOGGER = logging.getLogger(__name__)

REQUEST_TIMEOUT = aiohttp.ClientTimeout(total=10)


class CtrlmateConfigFlow(ConfigFlow, domain=DOMAIN):
    """Handle discovery, pairing and credential maintenance."""

    VERSION = 1

    def __init__(self) -> None:
        self._host = ""
        self._port = DEFAULT_PORT
        self._name = "Ctrlmate PC"
        self._fingerprint: str | None = None
        self._fingerprint_trusted = False
        self._reauth_entry: ConfigEntry | None = None
        self._reconfigure_entry: ConfigEntry | None = None

    @staticmethod
    @callback
    def async_get_options_flow(config_entry: ConfigEntry) -> CtrlmateOptionsFlow:
        return CtrlmateOptionsFlow()

    # ------------------------------------------------------------ network

    async def _async_read_fingerprint(self) -> str:
        """Raw TLS handshake: SHA-256 of the certificate the host presents."""
        return await async_get_peer_fingerprint(self._host, self._port)

    async def _async_fetch_info(self, fingerprint: str) -> dict[str, Any]:
        """Best-effort ``/api/v1/info`` over a pinned connection (minimal until paired)."""
        session = async_get_clientsession(self.hass)
        url = f"https://{format_host(self._host)}:{self._port}/api/v1/info"
        try:
            async with session.get(
                url, ssl=pinned_ssl(fingerprint), timeout=REQUEST_TIMEOUT
            ) as resp:
                if resp.status != 200:
                    return {}
                data = await resp.json()
        except (aiohttp.ClientError, TimeoutError, ValueError):
            return {}
        return data if isinstance(data, dict) else {}

    async def _async_pair(self, pin: str) -> tuple[dict[str, Any] | None, str | None]:
        """Exchange the PIN for a token over the pinned connection. Returns (data, error)."""
        assert self._fingerprint is not None
        session = async_get_clientsession(self.hass)
        url = f"https://{format_host(self._host)}:{self._port}/api/v1/pair"
        payload = {
            "pin": pin,
            "client_name": (self.hass.config.location_name or "Home Assistant")[:64],
        }
        try:
            async with session.post(
                url,
                json=payload,
                ssl=pinned_ssl(self._fingerprint),
                timeout=REQUEST_TIMEOUT,
            ) as resp:
                if resp.status == 401:
                    return None, "invalid_pin"
                if resp.status == 429:
                    return None, "too_many_attempts"
                if resp.status != 200:
                    return None, "cannot_connect"
                data = await resp.json()
        except aiohttp.ServerFingerprintMismatch:
            return None, "fingerprint_changed"
        except (aiohttp.ClientError, TimeoutError, ValueError):
            _LOGGER.debug("Pairing request failed", exc_info=True)
            return None, "cannot_connect"

        required = ("client_id", "token", "device_id")
        if not isinstance(data, dict) or any(not data.get(key) for key in required):
            return None, "cannot_connect"
        reported = normalize_fingerprint(data.get("cert_fingerprint"))
        if reported is not None and reported != self._fingerprint:
            return None, "fingerprint_changed"
        return data, None

    # ---------------------------------------------------------- discovery

    async def async_step_zeroconf(self, discovery_info: ZeroconfServiceInfo) -> ConfigFlowResult:
        """Handle mDNS discovery; the advertised ``fp`` is a hint verified against TLS."""
        self._host = discovery_info.host
        self._port = discovery_info.port or DEFAULT_PORT
        props = discovery_info.properties or {}
        device_id = props.get("id")
        if not device_id:
            return self.async_abort(reason="not_ctrlmate_device")
        self._name = props.get("name") or discovery_info.name.split(".")[0]
        advertised = normalize_fingerprint(props.get("fp"))

        await self.async_set_unique_id(device_id)
        existing = self.hass.config_entries.async_entry_for_domain_unique_id(DOMAIN, device_id)
        if existing is not None:
            stored = normalize_fingerprint(existing.data.get(CONF_CERT_FINGERPRINT))
            if advertised is None or advertised == stored:
                # Same machine: keep the entry pointed at its current address.
                self._abort_if_unique_id_configured(
                    updates={CONF_HOST: self._host, CONF_PORT: self._port}
                )
            self._abort_if_unique_id_configured()

        self.context["title_placeholders"] = {"name": self._name}
        try:
            self._fingerprint = await self._async_read_fingerprint()
        except CtrlmateTlsError:
            return self.async_abort(reason="cannot_connect")
        if advertised is not None:
            if advertised != self._fingerprint:
                return self.async_abort(reason="fingerprint_mismatch")
            self._fingerprint_trusted = True
        return await self._async_next_after_probe()

    async def async_step_user(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        """Manual host entry."""
        errors: dict[str, str] = {}
        if user_input is not None:
            self._host = user_input[CONF_HOST].strip()
            self._port = int(user_input[CONF_PORT])
            self._name = self._host
            try:
                self._fingerprint = await self._async_read_fingerprint()
            except CtrlmateTlsError:
                errors["base"] = "cannot_connect"
            else:
                info = await self._async_fetch_info(self._fingerprint)
                if device_id := info.get("device_id"):
                    await self.async_set_unique_id(device_id)
                    self._abort_if_unique_id_configured(
                        updates={CONF_HOST: self._host, CONF_PORT: self._port}
                    )
                self._name = info.get("hostname") or self._host
                self._fingerprint_trusted = False
                return await self._async_next_after_probe()

        return self.async_show_form(
            step_id="user",
            data_schema=vol.Schema(
                {
                    vol.Required(CONF_HOST): str,
                    vol.Required(CONF_PORT, default=DEFAULT_PORT): vol.All(
                        vol.Coerce(int), vol.Range(min=1, max=65535)
                    ),
                }
            ),
            errors=errors,
        )

    async def _async_next_after_probe(self) -> ConfigFlowResult:
        if self._fingerprint_trusted:
            return await self.async_step_pair()
        return await self.async_step_confirm_fingerprint()

    # ------------------------------------------------- TOFU confirmation

    async def async_step_confirm_fingerprint(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        """Show the certificate fingerprint; the user compares it with the desktop app."""
        assert self._fingerprint is not None
        errors: dict[str, str] = {}
        if user_input is not None:
            if user_input.get(CONF_CONFIRM):
                self._fingerprint_trusted = True
                return await self._async_after_confirm()
            errors["base"] = "fingerprint_not_confirmed"

        return self.async_show_form(
            step_id="confirm_fingerprint",
            data_schema=vol.Schema({vol.Required(CONF_CONFIRM, default=False): bool}),
            description_placeholders={
                "name": self._name,
                "host": self._host,
                "fingerprint": format_fingerprint(self._fingerprint),
            },
            errors=errors,
        )

    async def _async_after_confirm(self) -> ConfigFlowResult:
        if self._reconfigure_entry is not None:
            return self._finish_reconfigure()
        return await self.async_step_pair()

    # ------------------------------------------------------------ pairing

    async def async_step_pair(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        """Exchange the 6-digit PIN for a token over the pinned connection."""
        assert self._fingerprint is not None and self._fingerprint_trusted
        errors: dict[str, str] = {}

        if user_input is not None:
            pin = str(user_input[CONF_PIN]).strip()
            if not (pin.isdigit() and len(pin) == 6):
                errors["base"] = "invalid_pin"
            else:
                data, error = await self._async_pair(pin)
                if error == "fingerprint_changed":
                    return self.async_abort(reason="fingerprint_mismatch")
                if error is not None or data is None:
                    errors["base"] = error or "cannot_connect"
                else:
                    return await self._async_finish_pairing(data)

        return self.async_show_form(
            step_id="pair",
            data_schema=vol.Schema({vol.Required(CONF_PIN): str}),
            description_placeholders={"name": self._name, "host": self._host},
            errors=errors,
        )

    async def _async_finish_pairing(self, data: dict[str, Any]) -> ConfigFlowResult:
        device_id: str = data["device_id"]
        entry_data = {
            CONF_HOST: self._host,
            CONF_PORT: self._port,
            CONF_DEVICE_ID: device_id,
            CONF_CLIENT_ID: data["client_id"],
            CONF_TOKEN: data["token"],
            CONF_MAC_ADDRESS: data.get("mac_address"),
            CONF_CERT_FINGERPRINT: self._fingerprint,
        }

        if self._reauth_entry is not None:
            if device_id != self._reauth_entry.unique_id:
                return self.async_abort(reason="wrong_device")
            return self.async_update_reload_and_abort(
                self._reauth_entry,
                data_updates={
                    key: entry_data[key]
                    for key in (
                        CONF_HOST,
                        CONF_PORT,
                        CONF_CLIENT_ID,
                        CONF_TOKEN,
                        CONF_CERT_FINGERPRINT,
                    )
                },
            )

        # Normally decided before pairing (zeroconf id / info); this is the fallback.
        if self.unique_id is None:
            await self.async_set_unique_id(device_id)
        elif self.unique_id != device_id:
            return self.async_abort(reason="wrong_device")
        self._abort_if_unique_id_configured(updates={CONF_HOST: self._host, CONF_PORT: self._port})
        return self.async_create_entry(title=data.get("hostname") or self._name, data=entry_data)

    # -------------------------------------------------------------- reauth

    async def async_step_reauth(self, entry_data: Mapping[str, Any]) -> ConfigFlowResult:
        self._reauth_entry = self._get_reauth_entry()
        self._host = self._reauth_entry.data[CONF_HOST]
        self._port = self._reauth_entry.data[CONF_PORT]
        self._name = self._reauth_entry.title
        await self.async_set_unique_id(self._reauth_entry.unique_id)
        return await self.async_step_reauth_confirm()

    async def async_step_reauth_confirm(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        assert self._reauth_entry is not None
        errors: dict[str, str] = {}
        if user_input is not None:
            try:
                self._fingerprint = await self._async_read_fingerprint()
            except CtrlmateTlsError:
                errors["base"] = "cannot_connect"
            else:
                stored = normalize_fingerprint(self._reauth_entry.data.get(CONF_CERT_FINGERPRINT))
                # Unchanged certificate: still pinned. Changed: ask the user again.
                self._fingerprint_trusted = self._fingerprint == stored
                return await self._async_next_after_probe()

        return self.async_show_form(
            step_id="reauth_confirm",
            data_schema=vol.Schema({}),
            description_placeholders={"name": self._name, "host": self._host},
            errors=errors,
        )

    # ---------------------------------------------------------- reconfigure

    async def async_step_reconfigure(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        entry = self._get_reconfigure_entry()
        self._reconfigure_entry = entry
        self._name = entry.title
        errors: dict[str, str] = {}

        if user_input is not None:
            self._host = user_input[CONF_HOST].strip()
            self._port = int(user_input[CONF_PORT])
            try:
                self._fingerprint = await self._async_read_fingerprint()
            except CtrlmateTlsError:
                errors["base"] = "cannot_connect"
            else:
                info = await self._async_fetch_info(self._fingerprint)
                if (reported := info.get("device_id")) and reported != entry.unique_id:
                    errors["base"] = "wrong_device"
                else:
                    stored = normalize_fingerprint(entry.data.get(CONF_CERT_FINGERPRINT))
                    if self._fingerprint == stored:
                        return self._finish_reconfigure()
                    self._fingerprint_trusted = False
                    return await self.async_step_confirm_fingerprint()

        return self.async_show_form(
            step_id="reconfigure",
            data_schema=vol.Schema(
                {
                    vol.Required(
                        CONF_HOST,
                        default=(user_input or entry.data)[CONF_HOST],
                    ): str,
                    vol.Required(
                        CONF_PORT,
                        default=(user_input or entry.data)[CONF_PORT],
                    ): vol.All(vol.Coerce(int), vol.Range(min=1, max=65535)),
                }
            ),
            errors=errors,
        )

    def _finish_reconfigure(self) -> ConfigFlowResult:
        assert self._reconfigure_entry is not None and self._fingerprint is not None
        return self.async_update_reload_and_abort(
            self._reconfigure_entry,
            data_updates={
                CONF_HOST: self._host,
                CONF_PORT: self._port,
                CONF_CERT_FINGERPRINT: self._fingerprint,
            },
        )


class CtrlmateOptionsFlow(OptionsFlow):
    """Wake-on-LAN settings."""

    async def async_step_init(self, user_input: dict[str, Any] | None = None) -> ConfigFlowResult:
        errors: dict[str, str] = {}
        current = self.config_entry.options
        if user_input is not None:
            broadcast = (user_input.get(CONF_WOL_BROADCAST) or "").strip()
            try:
                if broadcast:
                    ipaddress.IPv4Address(broadcast)
            except ValueError:
                errors[CONF_WOL_BROADCAST] = "invalid_broadcast"
            else:
                return self.async_create_entry(
                    data={
                        CONF_WOL_BROADCAST: broadcast,
                        CONF_WOL_PORT: int(user_input[CONF_WOL_PORT]),
                    }
                )

        return self.async_show_form(
            step_id="init",
            data_schema=vol.Schema(
                {
                    vol.Optional(
                        CONF_WOL_BROADCAST,
                        default=(user_input or current).get(CONF_WOL_BROADCAST, ""),
                    ): str,
                    vol.Required(
                        CONF_WOL_PORT,
                        default=(user_input or current).get(CONF_WOL_PORT, DEFAULT_WOL_PORT),
                    ): vol.All(vol.Coerce(int), vol.Range(min=1, max=65535)),
                }
            ),
            errors=errors,
        )
