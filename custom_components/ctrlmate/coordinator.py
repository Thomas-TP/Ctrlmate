"""Push coordinator: pinned-fingerprint TLS WebSocket client and Wake-on-LAN."""

from __future__ import annotations

import asyncio
from collections.abc import Callable
import ipaddress
import logging
import random
import socket
from typing import TYPE_CHECKING, Any
import uuid

import aiohttp
from awesomeversion import AwesomeVersion, AwesomeVersionException

from homeassistant.components import network
from homeassistant.config_entries import ConfigEntry
from homeassistant.const import CONF_HOST, CONF_PORT
from homeassistant.core import HomeAssistant, callback
from homeassistant.exceptions import ConfigEntryAuthFailed, HomeAssistantError
from homeassistant.helpers import (
    device_registry as dr,
    entity_registry as er,
    issue_registry as ir,
)
from homeassistant.helpers.aiohttp_client import async_get_clientsession
from homeassistant.helpers.device_registry import CONNECTION_NETWORK_MAC
from homeassistant.helpers.update_coordinator import DataUpdateCoordinator

from .const import (
    BACKOFF_MAX,
    BACKOFF_MIN,
    COMMAND_TIMEOUT,
    CONF_CERT_FINGERPRINT,
    CONF_DEVICE_ID,
    CONF_MAC_ADDRESS,
    CONF_TOKEN,
    CONF_WOL_BROADCAST,
    CONF_WOL_PORT,
    CONNECT_TIMEOUT,
    DEFAULT_PORT,
    DEFAULT_WOL_PORT,
    DOMAIN,
    ISSUE_NO_AGENT,
    ISSUE_SERVICE_OUTDATED,
    RESULT_CODES,
)
from .tls import format_host, normalize_fingerprint, pinned_ssl

if TYPE_CHECKING:
    from . import CtrlmateConfigEntry

_LOGGER = logging.getLogger(__name__)

MIN_SERVICE_VERSION = "0.6.0"

# Entities the integration itself creates (not described one-to-one by the manifest).
STATIC_UNIQUE_SUFFIXES = ("wake_on_lan", "desktop_notify", "os_pending_updates")


class CtrlmateAuthError(Exception):
    """The daemon rejected our token (revoked or unknown)."""


class CtrlmateFingerprintError(Exception):
    """The presented certificate does not match the pinned fingerprint."""


def normalize_mac(mac: str | None) -> str | None:
    """Return a lower-case colon separated MAC or None when invalid."""
    if not mac:
        return None
    cleaned = mac.replace(":", "").replace("-", "").replace(".", "").strip().lower()
    if len(cleaned) != 12:
        return None
    try:
        bytes.fromhex(cleaned)
    except ValueError:
        return None
    return ":".join(cleaned[i : i + 2] for i in range(0, 12, 2))


def send_wol_magic_packet(mac_address: str, broadcast: str, port: int) -> None:
    """Send an IEEE 802.3 Wake-on-LAN magic packet over UDP (blocking)."""
    mac = normalize_mac(mac_address)
    if mac is None:
        raise ValueError(f"Invalid MAC address: {mac_address}")
    packet = b"\xff" * 6 + bytes.fromhex(mac.replace(":", "")) * 16
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        sock.sendto(packet, (broadcast, port))


async def async_default_broadcast(hass: HomeAssistant, host: str) -> str:
    """Return the directed broadcast address of the subnet that contains ``host``."""
    try:
        target = ipaddress.ip_address(host.strip("[]"))
    except ValueError:
        return "255.255.255.255"
    if target.version != 4:
        return "255.255.255.255"
    try:
        adapters = await network.async_get_adapters(hass)
    except Exception:
        return "255.255.255.255"
    for adapter in adapters:
        for ipv4 in adapter["ipv4"]:
            net = ipaddress.ip_network(f"{ipv4['address']}/{ipv4['network_prefix']}", strict=False)
            if target in net:
                return str(net.broadcast_address)
    return "255.255.255.255"


def backoff_delay(attempt: int, rand: Callable[[], float] = random.random) -> float:
    """Exponential backoff with jitter (50 %..100 % of the nominal delay)."""
    nominal = min(BACKOFF_MAX, BACKOFF_MIN * (2 ** max(0, attempt)))
    return nominal * (0.5 + 0.5 * rand())


class CtrlmateCoordinator(DataUpdateCoordinator[dict[str, dict[str, Any]]]):
    """Owns the push connection, the manifest and the entity state cache."""

    config_entry: CtrlmateConfigEntry

    def __init__(self, hass: HomeAssistant, entry: ConfigEntry) -> None:
        super().__init__(
            hass,
            _LOGGER,
            config_entry=entry,
            name=f"Ctrlmate {entry.title}",
            update_interval=None,
        )
        self.entry = entry
        self.device_id: str = entry.data[CONF_DEVICE_ID]
        self.manifest: dict[str, Any] = {}
        self.descriptors: dict[str, dict[str, Any]] = {}
        self.data = {}
        self.connected = False
        self.auth_failed = False
        self.parent_device_id: str | None = None
        self.hello: dict[str, Any] = {}

        self._ws: aiohttp.ClientWebSocketResponse | None = None
        self._task: asyncio.Task[None] | None = None
        self._settled = asyncio.Event()
        self._manifest_listeners: set[Callable[[], None]] = set()
        self._pending: dict[str, asyncio.Future[dict[str, Any]]] = {}
        self._rand: Callable[[], float] = random.random

    # ------------------------------------------------------------------ props

    @property
    def host(self) -> str:
        return self.entry.data[CONF_HOST]

    @property
    def port(self) -> int:
        return self.entry.data.get(CONF_PORT, DEFAULT_PORT)

    @property
    def mac_address(self) -> str | None:
        return normalize_mac(self.entry.data.get(CONF_MAC_ADDRESS))

    @property
    def states(self) -> dict[str, dict[str, Any]]:
        return self.data

    async def _async_update_data(self) -> dict[str, dict[str, Any]]:
        return self.data

    # -------------------------------------------------------------- listeners

    def add_manifest_listener(self, cb: Callable[[], None]) -> Callable[[], None]:
        self._manifest_listeners.add(cb)
        return lambda: self._manifest_listeners.discard(cb)

    # -------------------------------------------------------------- lifecycle

    async def async_start(self) -> None:
        """Start the connection loop and wait briefly for the first outcome.

        Raises ``ConfigEntryAuthFailed`` when the token is already revoked. An
        offline PC is not an error: entities become available when it comes up and
        Wake-on-LAN keeps working.
        """
        self._register_parent_device()
        self._task = self.entry.async_create_background_task(
            self.hass, self._run(), f"ctrlmate_ws_{self.device_id}"
        )
        try:
            async with asyncio.timeout(5):
                await self._settled.wait()
        except TimeoutError:
            _LOGGER.info("%s is offline; Wake-on-LAN remains available", self.entry.title)
        if self.auth_failed:
            raise ConfigEntryAuthFailed(translation_domain=DOMAIN, translation_key="auth_failed")

    async def async_stop(self) -> None:
        if self._task is not None:
            self._task.cancel()
            await asyncio.gather(self._task, return_exceptions=True)
            self._task = None
        await self._close_ws()
        self._fail_pending()
        ir.async_delete_issue(self.hass, DOMAIN, f"{ISSUE_NO_AGENT}_{self.device_id}")
        ir.async_delete_issue(self.hass, DOMAIN, f"{ISSUE_SERVICE_OUTDATED}_{self.device_id}")

    async def _close_ws(self) -> None:
        ws, self._ws = self._ws, None
        if ws is not None and not ws.closed:
            await ws.close()

    def _fail_pending(self) -> None:
        """Wake every command awaiting a result: the socket is gone."""
        for fut in self._pending.values():
            if not fut.done():
                fut.set_exception(ConnectionError("connection closed"))
        self._pending.clear()

    # ------------------------------------------------------------- connection

    async def _run(self) -> None:
        attempt = 0
        while True:
            try:
                reached_ready = await self._connect_once()
                attempt = 0 if reached_ready else attempt + 1
            except CtrlmateAuthError:
                _LOGGER.warning("Ctrlmate rejected the stored token for %s", self.entry.title)
                self._on_disconnected()
                self._start_reauth()
                return
            except CtrlmateFingerprintError:
                _LOGGER.error(
                    "Certificate of %s does not match the pinned fingerprint; "
                    "refusing the connection",
                    self.host,
                )
                self._on_disconnected()
                self._start_reauth()
                return
            except asyncio.CancelledError:
                self._on_disconnected()
                raise
            except (aiohttp.ClientError, TimeoutError, OSError, ValueError) as err:
                _LOGGER.debug("Connection to %s failed: %s", self.host, err)
                attempt += 1
            self._on_disconnected()
            await asyncio.sleep(backoff_delay(attempt, self._rand))

    def _start_reauth(self) -> None:
        self.auth_failed = True
        self._settled.set()
        self.entry.async_start_reauth(self.hass)

    def _on_disconnected(self) -> None:
        was_connected = self.connected
        self.connected = False
        self._ws = None
        self._fail_pending()
        if was_connected:
            self.async_update_listeners()

    async def _connect_once(self) -> bool:
        """Connect, authenticate and pump messages. Returns True once authenticated."""
        fingerprint = normalize_fingerprint(self.entry.data.get(CONF_CERT_FINGERPRINT))
        if fingerprint is None:
            raise CtrlmateFingerprintError("no pinned fingerprint stored")
        session = async_get_clientsession(self.hass)
        url = f"wss://{format_host(self.host)}:{self.port}/ws"
        try:
            ws = await session.ws_connect(
                url,
                ssl=pinned_ssl(fingerprint),
                heartbeat=30.0,
                timeout=aiohttp.ClientWSTimeout(ws_receive=None, ws_close=5.0),
            )
        except aiohttp.ServerFingerprintMismatch as err:
            raise CtrlmateFingerprintError(str(err)) from err

        ready = False
        try:
            self._ws = ws
            hello = await ws.receive_json(timeout=CONNECT_TIMEOUT)
            if hello.get("type") != "hello":
                raise ValueError("unexpected handshake frame")
            if hello.get("device_id") not in (None, self.device_id):
                raise ValueError("host presents a different device id")
            self.hello = hello
            await ws.send_json(
                {
                    "type": "auth",
                    "token": self.entry.data[CONF_TOKEN],
                    "client_name": (self.hass.config.location_name or "Home Assistant")[:64],
                }
            )
            reply = await ws.receive_json(timeout=CONNECT_TIMEOUT)
            if reply.get("type") == "auth_error":
                raise CtrlmateAuthError(str(reply.get("reason")))
            if reply.get("type") != "auth_ok":
                raise ValueError("unexpected auth reply")

            ready = True
            self.connected = True
            _LOGGER.info("Connected to Ctrlmate host '%s'", self.entry.title)
            self.async_update_listeners()
            async for msg in ws:
                if msg.type == aiohttp.WSMsgType.TEXT:
                    try:
                        self.handle_message(msg.json())
                    except ValueError:
                        _LOGGER.debug("Ignoring malformed frame from %s", self.host)
                elif msg.type in (
                    aiohttp.WSMsgType.CLOSE,
                    aiohttp.WSMsgType.CLOSING,
                    aiohttp.WSMsgType.CLOSED,
                    aiohttp.WSMsgType.ERROR,
                ):
                    break
        finally:
            if not ws.closed:
                await ws.close()
        return ready

    # --------------------------------------------------------------- messages

    @callback
    def handle_message(self, payload: dict[str, Any]) -> None:
        """Apply one decoded frame from the daemon."""
        msg_type = payload.get("type")
        if msg_type == "manifest":
            self._apply_manifest(payload)
        elif msg_type == "state_batch":
            states = dict(self.data)
            for item in payload.get("states", []):
                if isinstance(item, dict) and (ent_id := item.get("id")):
                    states[ent_id] = item
            self.data = states
            self.async_update_listeners()
        elif msg_type == "command_result":
            req_id = payload.get("request_id")
            fut = self._pending.get(req_id) if isinstance(req_id, str) else None
            if fut is not None and not fut.done():
                fut.set_result(payload)

    @callback
    def _apply_manifest(self, payload: dict[str, Any]) -> None:
        self.manifest = payload
        self.descriptors = {
            d["id"]: d for d in payload.get("entities", []) if isinstance(d, dict) and "id" in d
        }
        host = payload.get("host") or {}

        mac = normalize_mac(host.get("mac_address"))
        if mac and mac != self.mac_address:
            self.hass.config_entries.async_update_entry(
                self.entry, data={**self.entry.data, CONF_MAC_ADDRESS: mac}
            )
        self._register_parent_device()
        self._check_service_version(host.get("sw_version"))
        if payload.get("entities"):
            self._cleanup_stale()
        self._settled.set()
        for cb in list(self._manifest_listeners):
            cb()
        self.async_update_listeners()

    # ------------------------------------------------------------- registries

    def _register_parent_device(self) -> None:
        host = self.manifest.get("host", {})
        connections = {(CONNECTION_NETWORK_MAC, self.mac_address)} if self.mac_address else set()
        device = dr.async_get(self.hass).async_get_or_create(
            config_entry_id=self.entry.entry_id,
            identifiers={(DOMAIN, self.device_id)},
            connections=connections,
            name=host.get("name") or self.entry.title,
            manufacturer=host.get("manufacturer") or None,
            model=host.get("model") or None,
            sw_version=host.get("sw_version") or None,
        )
        self.parent_device_id = device.id

    def expected_unique_ids(self) -> set[str]:
        """Unique ids of every entity the current manifest justifies."""
        expected = {
            f"{self.device_id}_{d['id']}"
            for d in self.manifest.get("entities", [])
            if d.get("exposed", True)
        }
        expected.update(f"{self.device_id}_{s}" for s in STATIC_UNIQUE_SUFFIXES)
        return expected

    def expected_child_identifiers(self) -> set[tuple[str, str]]:
        return {
            (DOMAIN, f"{self.device_id}_{c['id']}") for c in self.manifest.get("child_devices", [])
        }

    @callback
    def _cleanup_stale(self) -> None:
        """Remove registry entries for hardware/entities that disappeared."""
        ent_reg = er.async_get(self.hass)
        expected = self.expected_unique_ids()
        for ent in er.async_entries_for_config_entry(ent_reg, self.entry.entry_id):
            if ent.unique_id not in expected:
                ent_reg.async_remove(ent.entity_id)

        dev_reg = dr.async_get(self.hass)
        keep = self.expected_child_identifiers() | {(DOMAIN, self.device_id)}
        # Child devices live in their own registry list in current Home Assistant.
        for child in dr.async_child_entries_for_config_entry(dev_reg, self.entry.entry_id):
            if not (child.identifiers & keep):
                dev_reg.async_remove_device(child.id)
        for dev in dr.async_entries_for_config_entry(dev_reg, self.entry.entry_id):
            if not (dev.identifiers & keep):
                dev_reg.async_update_device(dev.id, remove_config_entry_id=self.entry.entry_id)

    @callback
    def _check_service_version(self, sw_version: str | None) -> None:
        issue_id = f"{ISSUE_SERVICE_OUTDATED}_{self.device_id}"
        try:
            outdated = bool(sw_version) and AwesomeVersion(sw_version) < AwesomeVersion(
                MIN_SERVICE_VERSION
            )
        except AwesomeVersionException:
            outdated = False
        if not outdated:
            ir.async_delete_issue(self.hass, DOMAIN, issue_id)
            return
        ir.async_create_issue(
            self.hass,
            DOMAIN,
            issue_id,
            is_fixable=False,
            severity=ir.IssueSeverity.WARNING,
            translation_key=ISSUE_SERVICE_OUTDATED,
            translation_placeholders={
                "name": self.entry.title,
                "version": str(sw_version),
                "required": MIN_SERVICE_VERSION,
            },
        )

    # --------------------------------------------------------------- commands

    def _error(self, key: str, **placeholders: str) -> HomeAssistantError:
        return HomeAssistantError(
            translation_domain=DOMAIN,
            translation_key=key,
            translation_placeholders={"name": self.entry.title, **placeholders},
        )

    async def async_send_command(self, command: dict[str, Any]) -> dict[str, Any]:
        """Send a command and return the result frame, raising on any failure."""
        ws = self._ws
        if ws is None or ws.closed or not self.connected:
            raise self._error("pc_offline")

        req_id = uuid.uuid4().hex
        fut: asyncio.Future[dict[str, Any]] = asyncio.get_running_loop().create_future()
        self._pending[req_id] = fut
        try:
            await ws.send_json({"type": "command", "request_id": req_id, "command": command})
            async with asyncio.timeout(COMMAND_TIMEOUT):
                result = await fut
        except TimeoutError as err:
            raise self._error("command_timeout") from err
        except (ConnectionError, aiohttp.ClientError) as err:
            raise self._error("connection_lost") from err
        finally:
            self._pending.pop(req_id, None)

        self._raise_for_result(result)
        return result

    def _raise_for_result(self, result: dict[str, Any]) -> None:
        issue_id = f"{ISSUE_NO_AGENT}_{self.device_id}"
        if result.get("ok"):
            ir.async_delete_issue(self.hass, DOMAIN, issue_id)
            return
        code = result.get("code")
        if code not in RESULT_CODES:
            code = "internal"  # older daemons send no code
        if code == "no_agent":
            ir.async_create_issue(
                self.hass,
                DOMAIN,
                issue_id,
                is_fixable=False,
                severity=ir.IssueSeverity.WARNING,
                translation_key=ISSUE_NO_AGENT,
                translation_placeholders={"name": self.entry.title},
            )
        raise self._error(f"command_{code}", message=str(result.get("message") or ""))

    # ------------------------------------------------------------ Wake-on-LAN

    async def async_wake_pc(self) -> None:
        """Send a Wake-on-LAN packet; works whether or not the PC is connected."""
        mac = self.mac_address
        if mac is None:
            raise self._error("wol_no_mac")
        options = self.entry.options
        broadcast = options.get(CONF_WOL_BROADCAST) or await async_default_broadcast(
            self.hass, self.host
        )
        port = int(options.get(CONF_WOL_PORT, DEFAULT_WOL_PORT))
        try:
            ipaddress.IPv4Address(broadcast)
        except ValueError as err:
            raise self._error("wol_invalid_address", address=str(broadcast)) from err
        try:
            await self.hass.async_add_executor_job(send_wol_magic_packet, mac, broadcast, port)
        except OSError as err:
            raise self._error("wol_failed", message=str(err)) from err
