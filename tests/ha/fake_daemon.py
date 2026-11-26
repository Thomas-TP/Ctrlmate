"""A real TLS server that speaks the ctrlmated LAN protocol, for integration tests."""

from __future__ import annotations

import copy
import datetime as dt
import hashlib
import json
from pathlib import Path
import ssl
import tempfile
from typing import Any

from aiohttp import WSMsgType, web
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

FIXTURES = Path(__file__).parent / "fixtures"
DEVICE_ID = "pc-test-0001"
PIN = "123456"
TOKEN = "tok-" + "a" * 60


def load_fixture(name: str) -> dict[str, Any]:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def _make_certificate(directory: Path) -> tuple[Path, Path, str]:
    key = ec.generate_private_key(ec.SECP256R1())
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "ctrlmate-test")])
    now = dt.datetime.now(dt.UTC)
    cert = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(name)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - dt.timedelta(days=1))
        .not_valid_after(now + dt.timedelta(days=30))
        .sign(key, hashes.SHA256())
    )
    cert_path = directory / "cert.pem"
    key_path = directory / "key.pem"
    cert_path.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    key_path.write_bytes(
        key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        )
    )
    fingerprint = hashlib.sha256(cert.public_bytes(serialization.Encoding.DER)).hexdigest()
    return cert_path, key_path, fingerprint


class FakeDaemon:
    """Minimal ctrlmated: /api/v1/info, /api/v1/pair and /ws over TLS 1.3."""

    def __init__(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        cert, key, self.fingerprint = _make_certificate(Path(self._tmp.name))
        self._ssl = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self._ssl.minimum_version = ssl.TLSVersion.TLSv1_3
        self._ssl.load_cert_chain(cert, key)

        self.manifest = load_fixture("manifest.json")
        self.states = load_fixture("states.json")
        self.expose_device_id_in_info = True
        self.valid_tokens = {TOKEN}
        self.command_results: dict[str, dict[str, Any]] = {}
        self.default_result: dict[str, Any] = {"ok": True, "code": "ok"}
        self.drop_on_command = False
        self.commands: list[dict[str, Any]] = []
        self.pair_attempts = 0
        self.sockets: list[web.WebSocketResponse] = []
        self.port = 0

        app = web.Application()
        app.router.add_get("/api/v1/info", self._info)
        app.router.add_post("/api/v1/pair", self._pair)
        app.router.add_get("/ws", self._ws)
        self._runner = web.AppRunner(app)

    async def start(self) -> FakeDaemon:
        await self._runner.setup()
        site = web.TCPSite(self._runner, "127.0.0.1", 0, ssl_context=self._ssl)
        await site.start()
        self.port = site._server.sockets[0].getsockname()[1]
        return self

    async def stop(self) -> None:
        for ws in list(self.sockets):
            await ws.close()
        await self._runner.cleanup()
        self._tmp.cleanup()

    async def broadcast(self, message: dict[str, Any]) -> None:
        for ws in list(self.sockets):
            await ws.send_json(message)

    def manifest_copy(self) -> dict[str, Any]:
        return copy.deepcopy(self.manifest)

    # ------------------------------------------------------------- handlers

    async def _info(self, request: web.Request) -> web.Response:
        data: dict[str, Any] = {"protocol_version": 1, "sw_version": "0.6.0"}
        if self.expose_device_id_in_info:
            data["device_id"] = DEVICE_ID
        return web.json_response(data)

    async def _pair(self, request: web.Request) -> web.Response:
        self.pair_attempts += 1
        body = await request.json()
        if body.get("pin") != PIN:
            return web.json_response({"error": "invalid pin"}, status=401)
        return web.json_response(
            {
                "client_id": "client-1",
                "token": TOKEN,
                "device_id": DEVICE_ID,
                "hostname": "Gaming PC",
                "mac_address": "AA:BB:CC:DD:EE:FF",
                "cert_fingerprint": self.fingerprint,
            }
        )

    async def _ws(self, request: web.Request) -> web.WebSocketResponse:
        ws = web.WebSocketResponse()
        await ws.prepare(request)
        await ws.send_json(
            {
                "type": "hello",
                "protocol_version": 1,
                "device_id": DEVICE_ID,
                "hostname": "Gaming PC",
                "sw_version": "0.6.0",
                "cert_fingerprint": self.fingerprint,
            }
        )
        try:
            auth = await ws.receive_json()
        except (TypeError, ValueError):
            return ws
        if auth.get("type") != "auth" or auth.get("token") not in self.valid_tokens:
            await ws.send_json({"type": "auth_error", "reason": "Invalid or revoked bearer token"})
            await ws.close()
            return ws
        await ws.send_json({"type": "auth_ok", "client_id": "client-1"})
        await ws.send_json(self.manifest)
        await ws.send_json(self.states)
        self.sockets.append(ws)
        try:
            async for msg in ws:
                if msg.type != WSMsgType.TEXT:
                    continue
                frame = msg.json()
                if frame.get("type") != "command":
                    continue
                command = frame["command"]
                self.commands.append(command)
                if self.drop_on_command:
                    await ws.close()
                    break
                result = self.command_results.get(command["action"], self.default_result)
                await ws.send_json(
                    {"type": "command_result", "request_id": frame["request_id"], **result}
                )
        finally:
            if ws in self.sockets:
                self.sockets.remove(ws)
        return ws
