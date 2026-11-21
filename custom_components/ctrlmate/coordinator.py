"""Persistent WSS Local Push client & Wake-on-LAN manager for Ctrlmate."""

from __future__ import annotations

import asyncio
import hashlib
import logging
import socket
import uuid
from collections.abc import Callable
from typing import Any

import aiohttp

from homeassistant.config_entries import ConfigEntry
from homeassistant.const import CONF_HOST, CONF_PORT
from homeassistant.core import HomeAssistant
from homeassistant.helpers.aiohttp_client import async_get_clientsession

from .const import (
    CONF_CERT_FINGERPRINT,
    CONF_DEVICE_ID,
    CONF_MAC_ADDRESS,
    CONF_TOKEN,
    DEFAULT_PORT,
)

_LOGGER = logging.getLogger(__name__)


def send_wol_magic_packet(mac_address: str, broadcast: str = "255.255.255.255", port: int = 9) -> None:
    """Send an IEEE 802.3 Wake-on-LAN magic packet over UDP."""
    cleaned = mac_address.replace(":", "").replace("-", "").replace(".", "").strip()
    if len(cleaned) != 12:
        raise ValueError(f"Invalid MAC address for Wake-on-LAN: {mac_address}")
    mac_bytes = bytes.fromhex(cleaned)
    packet = b"\xff" * 6 + mac_bytes * 16
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        sock.sendto(packet, (broadcast, port))


class CtrlmateCoordinator:
    """Manages the persistent TLS WebSocket push stream and entity state cache."""

    def __init__(self, hass: HomeAssistant, entry: ConfigEntry) -> None:
        self.hass = hass
        self.entry = entry
        self.host: str = entry.data[CONF_HOST]
        self.port: int = entry.data.get(CONF_PORT, DEFAULT_PORT)
        self.device_id: str = entry.data[CONF_DEVICE_ID]
        self.token: str = entry.data[CONF_TOKEN]
        self.mac_address: str | None = entry.data.get(CONF_MAC_ADDRESS)
        self.expected_fp: str = entry.data.get(CONF_CERT_FINGERPRINT, "").lower()

        self.connected: bool = False
        self.manifest: dict[str, Any] = {}
        self.states: dict[str, dict[str, Any]] = {}

        self._ws: aiohttp.ClientWebSocketResponse | None = None
        self._running: bool = False
        self._initial_manifest_event = asyncio.Event()
        self._listeners: set[Callable[[], None]] = set()
        self._manifest_listeners: set[Callable[[], None]] = set()
        self._pending_commands: dict[str, asyncio.Future[dict[str, Any]]] = {}

    def add_state_listener(self, callback: Callable[[], None]) -> Callable[[], None]:
        self._listeners.add(callback)
        return lambda: self._listeners.discard(callback)

    def add_manifest_listener(self, callback: Callable[[], None]) -> Callable[[], None]:
        self._manifest_listeners.add(callback)
        return lambda: self._manifest_listeners.discard(callback)

    def _notify_state_listeners(self) -> None:
        for cb in list(self._listeners):
            cb()

    def _notify_manifest_listeners(self) -> None:
        for cb in list(self._manifest_listeners):
            cb()

    async def async_start(self) -> None:
        """Start the background WebSocket loop and wait briefly for the first manifest."""
        self._running = True
        self.entry.async_create_background_task(
            self.hass, self._ws_loop(), f"ctrlmate_ws_{self.device_id}"
        )
        try:
            await asyncio.wait_for(self._initial_manifest_event.wait(), timeout=5.0)
        except TimeoutError:
            _LOGGER.info(
                "Ctrlmate PC '%s' is currently offline; Wake-on-LAN remains available",
                self.entry.title,
            )

    async def async_stop(self) -> None:
        self._running = False
        if self._ws is not None and not self._ws.closed:
            await self._ws.close()
        self.connected = False
        self._notify_state_listeners()

    async def async_send_command(self, command: dict[str, Any]) -> dict[str, Any]:
        """Send a control command to ctrlmated and await its acknowledgement."""
        if self._ws is None or self._ws.closed or not self.connected:
            raise ConnectionError("Ctrlmate PC is offline")

        req_id = str(uuid.uuid4())
        loop = asyncio.get_running_loop()
        fut: asyncio.Future[dict[str, Any]] = loop.create_future()
        self._pending_commands[req_id] = fut

        try:
            await self._ws.send_json(
                {
                    "type": "command",
                    "request_id": req_id,
                    "command": command,
                }
            )
            return await asyncio.wait_for(fut, timeout=10.0)
        finally:
            self._pending_commands.pop(req_id, None)

    async def async_wake_pc(self) -> None:
        """Send a Wake-on-LAN magic packet to power on the PC."""
        if not self.mac_address:
            raise ValueError("No MAC address registered for this Ctrlmate host")
        await self.hass.async_add_executor_job(send_wol_magic_packet, self.mac_address)

    def _verify_tofu_fingerprint(self, ws: aiohttp.ClientWebSocketResponse) -> bool:
        """Verify peer TLS certificate SHA-256 fingerprint pinned during pairing."""
        if not self.expected_fp:
            return True
        try:
            transport = ws._response.connection.transport  # noqa: SLF001
            ssl_obj = transport.get_extra_info("ssl_object") if transport else None
            if ssl_obj is None:
                return False
            der = ssl_obj.getpeercert(binary_form=True)
            if not der:
                return False
            actual_fp = hashlib.sha256(der).hexdigest().lower()
            return actual_fp == self.expected_fp
        except Exception:
            _LOGGER.debug("Could not verify TLS TOFU fingerprint", exc_info=True)
            return False

    async def _ws_loop(self) -> None:
        session = async_get_clientsession(self.hass, verify_ssl=False)
        url = f"wss://{self.host}:{self.port}/ws"
        backoff = 2.0

        while self._running:
            try:
                async with session.ws_connect(
                    url,
                    heartbeat=30.0,
                    timeout=aiohttp.ClientWSTimeout(ws_receive=None, ws_close=5.0),
                ) as ws:
                    if not self._verify_tofu_fingerprint(ws):
                        _LOGGER.error(
                            "TLS certificate fingerprint mismatch for Ctrlmate host %s — refusing connection",
                            self.host,
                        )
                        await asyncio.sleep(30.0)
                        continue

                    self._ws = ws
                    backoff = 2.0

                    # 1. Wait for Hello banner
                    hello = await ws.receive_json(timeout=10.0)
                    if hello.get("type") != "hello":
                        raise ConnectionError(f"Unexpected handshake frame: {hello}")

                    # 2. Authenticate using paired bearer token
                    await ws.send_json(
                        {
                            "type": "auth",
                            "token": self.token,
                            "client_name": self.hass.config.location_name or "Home Assistant",
                        }
                    )
                    auth_reply = await ws.receive_json(timeout=10.0)
                    if auth_reply.get("type") != "auth_ok":
                        _LOGGER.error("Ctrlmate auth rejected: %s", auth_reply.get("reason"))
                        await asyncio.sleep(30.0)
                        continue

                    self.connected = True
                    _LOGGER.info("Connected to Ctrlmate host '%s' (%s)", self.entry.title, self.host)
                    self._notify_state_listeners()

                    async for msg in ws:
                        if msg.type == aiohttp.WSMsgType.TEXT:
                            self._handle_message(msg.json())
                        elif msg.type in (aiohttp.WSMsgType.CLOSED, aiohttp.WSMsgType.ERROR):
                            break
            except asyncio.CancelledError:
                break
            except Exception as err:
                _LOGGER.debug("Ctrlmate WS connection to %s interrupted: %s", self.host, err)
            finally:
                was_connected = self.connected
                self.connected = False
                self._ws = None
                if was_connected:
                    self._notify_state_listeners()

            if self._running:
                await asyncio.sleep(backoff)
                backoff = min(backoff * 1.5, 30.0)

    def _handle_message(self, payload: dict[str, Any]) -> None:
        msg_type = payload.get("type")

        if msg_type == "manifest":
            self.manifest = payload
            host_mac = payload.get("host", {}).get("mac_address")
            if host_mac:
                self.mac_address = host_mac
            self._initial_manifest_event.set()
            self._notify_manifest_listeners()
            self._notify_state_listeners()
        elif msg_type == "state_batch":
            for item in payload.get("states", []):
                ent_id = item.get("id")
                if ent_id:
                    self.states[ent_id] = item
            self._notify_state_listeners()
        elif msg_type == "command_result":
            req_id = payload.get("request_id")
            if req_id and (fut := self._pending_commands.get(req_id)) and not fut.done():
                fut.set_result(payload)
