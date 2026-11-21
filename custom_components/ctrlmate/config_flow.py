"""Config flow for Ctrlmate (Zeroconf + 6-digit PIN + TLS 1.3 TOFU pairing)."""

from __future__ import annotations

import logging
from typing import Any

import aiohttp
import voluptuous as vol

from homeassistant.config_entries import ConfigFlow, ConfigFlowResult
from homeassistant.const import CONF_HOST, CONF_PORT
from homeassistant.helpers.aiohttp_client import async_get_clientsession
from homeassistant.helpers.service_info.zeroconf import ZeroconfServiceInfo

from .const import (
    CONF_CERT_FINGERPRINT,
    CONF_CLIENT_ID,
    CONF_DEVICE_ID,
    CONF_MAC_ADDRESS,
    CONF_PIN,
    CONF_TOKEN,
    DEFAULT_PORT,
    DOMAIN,
)

_LOGGER = logging.getLogger(__name__)


class CtrlmateConfigFlow(ConfigFlow, domain=DOMAIN):
    """Handle Ctrlmate discovery and modern 6-digit PIN pairing."""

    VERSION = 1

    def __init__(self) -> None:
        self._host: str = ""
        self._port: int = DEFAULT_PORT
        self._hostname: str = "Ctrlmate PC"
        self._device_id: str | None = None
        self._cert_fingerprint: str = ""

    async def async_step_zeroconf(
        self, discovery_info: ZeroconfServiceInfo
    ) -> ConfigFlowResult:
        """Handle automatic mDNS discovery on the local network."""
        self._host = discovery_info.host
        self._port = discovery_info.port or DEFAULT_PORT
        props = discovery_info.properties or {}
        self._device_id = props.get("id")
        self._hostname = props.get("name") or discovery_info.name.split(".")[0]

        if self._device_id:
            await self.async_set_unique_id(self._device_id)
            self._abort_if_unique_id_configured(
                updates={CONF_HOST: self._host, CONF_PORT: self._port}
            )

        self.context["title_placeholders"] = {"name": self._hostname}
        return await self.async_step_pair()

    async def async_step_user(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        """Handle manual host/port entry."""
        errors: dict[str, str] = {}

        if user_input is not None:
            self._host = user_input[CONF_HOST].strip()
            self._port = int(user_input[CONF_PORT])
            try:
                info = await self._fetch_info(self._host, self._port)
            except Exception:
                _LOGGER.debug("Failed connecting to Ctrlmate at %s:%s", self._host, self._port, exc_info=True)
                errors["base"] = "cannot_connect"
            else:
                self._device_id = info["device_id"]
                self._hostname = info.get("hostname", self._host)
                self._cert_fingerprint = info.get("cert_fingerprint", "")
                await self.async_set_unique_id(self._device_id)
                self._abort_if_unique_id_configured(
                    updates={CONF_HOST: self._host, CONF_PORT: self._port}
                )
                return await self.async_step_pair()

        return self.async_show_form(
            step_id="user",
            data_schema=vol.Schema(
                {
                    vol.Required(CONF_HOST): str,
                    vol.Required(CONF_PORT, default=DEFAULT_PORT): int,
                }
            ),
            errors=errors,
        )

    async def async_step_pair(
        self, user_input: dict[str, Any] | None = None
    ) -> ConfigFlowResult:
        """Exchange the temporary 6-digit PIN shown in Ctrlmate for a signed token."""
        errors: dict[str, str] = {}

        if user_input is not None:
            pin = str(user_input[CONF_PIN]).strip()
            session = async_get_clientsession(self.hass, verify_ssl=False)
            url = f"https://{self._host}:{self._port}/api/v1/pair"
            payload = {
                "pin": pin,
                "client_name": self.hass.config.location_name or "Home Assistant",
            }
            try:
                async with session.post(
                    url, json=payload, timeout=aiohttp.ClientTimeout(total=10)
                ) as resp:
                    if resp.status == 401:
                        errors["base"] = "invalid_pin"
                    elif resp.status != 200:
                        errors["base"] = "cannot_connect"
                    else:
                        data = await resp.json()
                        await self.async_set_unique_id(data["device_id"])
                        self._abort_if_unique_id_configured()
                        return self.async_create_entry(
                            title=data.get("hostname", self._hostname),
                            data={
                                CONF_HOST: self._host,
                                CONF_PORT: self._port,
                                CONF_DEVICE_ID: data["device_id"],
                                CONF_CLIENT_ID: data["client_id"],
                                CONF_TOKEN: data["token"],
                                CONF_MAC_ADDRESS: data.get("mac_address"),
                                CONF_CERT_FINGERPRINT: data["cert_fingerprint"],
                            },
                        )
            except Exception:
                _LOGGER.debug("Pairing request failed", exc_info=True)
                errors["base"] = "cannot_connect"

        return self.async_show_form(
            step_id="pair",
            description_placeholders={"name": self._hostname, "host": self._host},
            data_schema=vol.Schema({vol.Required(CONF_PIN): str}),
            errors=errors,
        )

    async def _fetch_info(self, host: str, port: int) -> dict[str, Any]:
        session = async_get_clientsession(self.hass, verify_ssl=False)
        url = f"https://{host}:{port}/api/v1/info"
        async with session.get(url, timeout=aiohttp.ClientTimeout(total=8)) as resp:
            resp.raise_for_status()
            return await resp.json()
