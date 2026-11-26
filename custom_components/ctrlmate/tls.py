"""TLS helpers: trust-on-first-use fingerprint discovery and pinned connections."""

from __future__ import annotations

import asyncio
import hashlib
import ipaddress
import re
import ssl

import aiohttp

from .const import CONNECT_TIMEOUT
import contextlib

_FP_RE = re.compile(r"^[0-9a-f]{64}$")


class CtrlmateTlsError(Exception):
    """The TLS handshake with the Ctrlmate host failed."""


def normalize_fingerprint(value: str | None) -> str | None:
    """Return a lower-case 64-hex SHA-256 fingerprint, or None if invalid."""
    if not value:
        return None
    cleaned = value.replace(":", "").replace(" ", "").strip().lower()
    return cleaned if _FP_RE.match(cleaned) else None


def format_fingerprint(fingerprint: str) -> str:
    """Format a 64-hex fingerprint as upper-case colon separated pairs."""
    fp = fingerprint.upper()
    return ":".join(fp[i : i + 2] for i in range(0, len(fp), 2))


def format_host(host: str) -> str:
    """Return a host usable in a URL authority (IPv6 literals are bracketed)."""
    host = host.strip()
    if host.startswith("[") and host.endswith("]"):
        return host
    try:
        if isinstance(ipaddress.ip_address(host.split("%", 1)[0]), ipaddress.IPv6Address):
            return f"[{host}]"
    except ValueError:
        pass
    return host


def pinned_ssl(fingerprint: str) -> aiohttp.Fingerprint:
    """Return an aiohttp ``ssl`` argument pinning the SHA-256 of the peer certificate."""
    fp = normalize_fingerprint(fingerprint)
    if fp is None:
        raise ValueError("invalid certificate fingerprint")
    return aiohttp.Fingerprint(bytes.fromhex(fp))


async def async_get_peer_fingerprint(host: str, port: int) -> str:
    """Open a raw TLS connection and return the SHA-256 of the peer certificate.

    This is the only place certificate verification is skipped, and only to *read*
    the certificate that the user then has to confirm (trust on first use). No
    application data is sent on this connection.
    """
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.minimum_version = ssl.TLSVersion.TLSv1_3
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE

    writer: asyncio.StreamWriter | None = None
    try:
        async with asyncio.timeout(CONNECT_TIMEOUT):
            _reader, writer = await asyncio.open_connection(host.strip("[]"), port, ssl=context)
            ssl_object = writer.get_extra_info("ssl_object")
            der = ssl_object.getpeercert(binary_form=True) if ssl_object else None
    except (OSError, ssl.SSLError, TimeoutError) as err:
        raise CtrlmateTlsError(str(err)) from err
    finally:
        if writer is not None:
            writer.close()
            with contextlib.suppress(OSError, ssl.SSLError):
                await writer.wait_closed()
    if not der:
        raise CtrlmateTlsError("peer presented no certificate")
    return hashlib.sha256(der).hexdigest()
