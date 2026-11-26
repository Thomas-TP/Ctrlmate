"""Pure helpers and repository-level guarantees."""

from __future__ import annotations

import json
from pathlib import Path
import re

import pytest

from custom_components.ctrlmate.const import (
    CHILD_TRANSLATION_KEYS,
    COOLING_PRESETS,
    PARENT_TRANSLATION_KEYS,
    RESULT_CODES,
)
from custom_components.ctrlmate.coordinator import backoff_delay, normalize_mac
from custom_components.ctrlmate.tls import (
    format_fingerprint,
    format_host,
    normalize_fingerprint,
    pinned_ssl,
)

ROOT = Path(__file__).resolve().parents[2]
INTEGRATION = ROOT / "custom_components" / "ctrlmate"


def test_format_host_brackets_ipv6_only() -> None:
    assert format_host("192.168.1.2") == "192.168.1.2"
    assert format_host("pc.local") == "pc.local"
    assert format_host("fd00::1") == "[fd00::1]"
    assert format_host("[fd00::1]") == "[fd00::1]"


def test_fingerprint_helpers() -> None:
    fp = "ab" * 32
    assert normalize_fingerprint(format_fingerprint(fp)) == fp
    assert normalize_fingerprint("zz" * 32) is None
    assert normalize_fingerprint("abcd") is None
    assert normalize_fingerprint(None) is None
    with pytest.raises(ValueError):
        pinned_ssl("short")


def test_normalize_mac() -> None:
    assert normalize_mac("AA-BB-CC-DD-EE-FF") == "aa:bb:cc:dd:ee:ff"
    assert normalize_mac("nope") is None
    assert normalize_mac(None) is None


def test_backoff_has_jitter_and_cap() -> None:
    low = [backoff_delay(n, lambda: 0.0) for n in range(12)]
    high = [backoff_delay(n, lambda: 1.0) for n in range(12)]
    assert all(lo < hi for lo, hi in zip(low, high, strict=True))
    assert max(high) <= 60.0
    assert low == sorted(low)


def test_never_disables_certificate_verification_in_aiohttp() -> None:
    for path in INTEGRATION.glob("*.py"):
        text = path.read_text(encoding="utf-8")
        assert "verify_ssl" not in text, path
        assert "ssl=False" not in text, path
        assert "_response.connection" not in text, path  # no private attribute poking


def test_strings_en_identical_and_fr_complete() -> None:
    strings = json.loads((INTEGRATION / "strings.json").read_text(encoding="utf-8"))
    en = json.loads((INTEGRATION / "translations" / "en.json").read_text(encoding="utf-8"))
    fr = json.loads((INTEGRATION / "translations" / "fr.json").read_text(encoding="utf-8"))
    assert strings == en

    def keys(node: object, prefix: str = "") -> set[str]:
        if isinstance(node, dict):
            return {k for key, value in node.items() for k in keys(value, f"{prefix}{key}.")}
        return {prefix}

    assert keys(en) == keys(fr)


def test_translation_tables_are_covered() -> None:
    en = json.loads((INTEGRATION / "strings.json").read_text(encoding="utf-8"))
    icons = json.loads((INTEGRATION / "icons.json").read_text(encoding="utf-8"))
    sensor_keys = set(en["entity"]["sensor"])
    all_keys = set(PARENT_TRANSLATION_KEYS.values()) | set(CHILD_TRANSLATION_KEYS.values())
    flat = {k for domain in en["entity"].values() for k in domain}
    assert all_keys <= flat | {"lighting"}
    assert sensor_keys <= set(icons["entity"]["sensor"])
    for domain, mapping in en["entity"].items():
        assert set(mapping) <= set(icons["entity"][domain]), domain
    for code in RESULT_CODES:
        assert f"command_{code}" in en["exceptions"]
    assert set(en["entity"]["select"]["cooling_preset"]["state"]) == set(COOLING_PRESETS)


def test_manifest_and_hacs_are_consistent() -> None:
    manifest = json.loads((INTEGRATION / "manifest.json").read_text(encoding="utf-8"))
    hacs = json.loads((ROOT / "hacs.json").read_text(encoding="utf-8"))
    assert manifest["domain"] == "ctrlmate"
    assert manifest["version"] == "0.6.0"
    assert manifest["integration_type"] in {"hub", "device"}
    assert manifest["loggers"]
    assert manifest["config_flow"] is True
    assert re.fullmatch(r"\d{4}\.\d+\.\d+", hacs["homeassistant"])
    assert (INTEGRATION / "brand" / "icon.png").is_file()


def test_update_helpers_are_honest() -> None:
    from custom_components.ctrlmate.entity import (
        update_kb_ids,
        update_pending_count,
        update_titles,
    )

    # Current wire shape: no kb list, KB ids are parsed from titles.
    info = {
        "pending_count": 2,
        "pending_titles": ["2026-11 Cumulative Update (KB5050009)", "Definition kb2267602 update"],
    }
    assert update_pending_count(info) == 2
    assert update_kb_ids(info) == ["KB5050009", "KB2267602"]
    assert update_titles({"pending": [{"title": "A", "kb": "KB1234567"}]}) == ["A"]
    # Never scanned / failed scan: unknown, not zero.
    assert update_pending_count({"pending_count": None}) is None
    assert update_pending_count({"pending_count": 0, "checked": False}) is None
    assert update_pending_count({"pending_count": 0, "error": "unavailable"}) is None
    assert update_pending_count({"pending_count": 0}) == 0
