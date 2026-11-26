"""Shared fixtures for the Ctrlmate integration tests."""

from __future__ import annotations

from collections.abc import AsyncGenerator
from typing import Any

import pytest
from pytest_homeassistant_custom_component.common import MockConfigEntry

from custom_components.ctrlmate.const import (
    CONF_CERT_FINGERPRINT,
    CONF_CLIENT_ID,
    CONF_DEVICE_ID,
    CONF_MAC_ADDRESS,
    CONF_TOKEN,
    DOMAIN,
)
from homeassistant.const import CONF_HOST, CONF_PORT
from homeassistant.core import HomeAssistant

from .fake_daemon import DEVICE_ID, TOKEN, FakeDaemon


@pytest.fixture(autouse=True)
def auto_enable_custom_integrations(enable_custom_integrations: None) -> None:
    """Allow loading custom_components in every test."""


@pytest.fixture(autouse=True)
def _fast_backoff(monkeypatch: pytest.MonkeyPatch) -> None:
    """Keep reconnect attempts quick in tests."""
    monkeypatch.setattr("custom_components.ctrlmate.coordinator.BACKOFF_MIN", 0.05)
    monkeypatch.setattr("custom_components.ctrlmate.coordinator.BACKOFF_MAX", 0.2)


@pytest.fixture
async def daemon(socket_enabled: None) -> AsyncGenerator[FakeDaemon]:
    server = await FakeDaemon().start()
    yield server
    await server.stop()


def make_entry(daemon: FakeDaemon, **overrides: Any) -> MockConfigEntry:
    data = {
        CONF_HOST: "127.0.0.1",
        CONF_PORT: daemon.port,
        CONF_DEVICE_ID: DEVICE_ID,
        CONF_CLIENT_ID: "client-1",
        CONF_TOKEN: TOKEN,
        CONF_MAC_ADDRESS: "aa:bb:cc:dd:ee:ff",
        CONF_CERT_FINGERPRINT: daemon.fingerprint,
    }
    data.update(overrides)
    return MockConfigEntry(domain=DOMAIN, unique_id=DEVICE_ID, title="Gaming PC", data=data)


@pytest.fixture
async def setup_entry(hass: HomeAssistant, daemon: FakeDaemon) -> MockConfigEntry:
    """A fully set up, connected entry."""
    entry = make_entry(daemon)
    entry.add_to_hass(hass)
    assert await hass.config_entries.async_setup(entry.entry_id)
    await hass.async_block_till_done()
    return entry
