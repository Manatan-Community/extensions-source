#!/usr/bin/env python3
"""Generate Play-compatible compiled Rulate-family `.manatan2` leaves.

The generated packages contain only Rust/Wasm and sanitized data. They never
package or evaluate LNReader's TypeScript. Network permissions are restricted
to each source's exact HTTPS origin, and adult/unknown classifications are
enforced again by the shared Rust adapter at every content boundary.
"""

from __future__ import annotations

import copy
import json
import os
import re
import shutil
import tempfile
from pathlib import Path
from urllib.parse import urlparse

from lnreader_paths import plugins_checkout


ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = plugins_checkout()
FAMILY = UPSTREAM / "plugins/multisrc/rulate"
PUBLISHER_KEY = "88b67d201d387960b96b64b5c4ca39d5edceef6e8a088316449a2d5437a889ac"
API_KEY = "fpoiKLUues81werht039"
GENERATED_MARKER = "Generated from LNReader's MIT-licensed Rulate-family descriptor."

# A direct probe on 2026-07-19 returned Cloudflare 522. More importantly, the
# source's own genre taxonomy is an explicit-adult catalog, so it cannot be
# represented honestly by the fail-closed Play-safe adapter.
BLOCKED = {
    "erolate-api": (
        "The upstream returned HTTP 522 during a fresh 2026-07-19 probe and "
        "its source taxonomy is explicitly adult-only; no Play-safe leaf is generated."
    ),
}

BLOCKED_TERMS = (
    "18+", "21+", "adult", "anal", "bdsm", "ecchi", "erotic", "explicit",
    "hentai", "incest", "milf", "netorare", "oral sex", "porn", "rape",
    "sex", "smut", "аналь", "бдсм", "изнасил", "инцест", "нетораре",
    "порно", "секс", "смат", "хентай", "эрот", "этти", "эччи",
)


def canonical_id(value: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")


def rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def blocked_option(value: str) -> bool:
    normalized = " ".join(value.lower().replace("_", " ").replace("-", " ").split())
    return any(term in normalized for term in BLOCKED_TERMS)


def sanitized_filters(source: dict) -> dict:
    payload = copy.deepcopy(json.loads((FAMILY / "settings.json").read_text()))
    source_filters = FAMILY / "filters" / f"{source['sourceName']}.json"
    if source_filters.is_file():
        payload.setdefault("filters", {}).update(
            json.loads(source_filters.read_text()).get("filters", {})
        )
    for definition in payload.get("filters", {}).values():
        definition["options"] = [
            option
            for option in definition.get("options", [])
            if str(option.get("value", "")).isdigit()
            and not blocked_option(str(option.get("label", "")))
        ]
    return payload


def source_lib(source: dict, source_id: str, base_url: str) -> str:
    return f'''// {GENERATED_MARKER}
// Runtime behavior is compiled Rust; no source code is downloaded or evaluated.

use rulate_novel::RulateConfig;

#[derive(Default)]
pub struct SourceConfig;

impl RulateConfig for SourceConfig {{
    const NAME: &'static str = {rust_string(source["sourceName"])};
    const BASE_URL: &'static str = {rust_string(base_url)};
    const API_KEY: &'static str = {rust_string(API_KEY)};
    const FILTERS_JSON: &'static str = include_str!("../filters.json");
}}

rulate_novel::export_rulate_extension!(SourceConfig, {rust_string(source_id)});

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn preserves_exact_compiled_identity_and_safe_filters() {{
        assert_eq!(SourceConfig::NAME, {rust_string(source["sourceName"])});
        assert_eq!(SourceConfig::BASE_URL, {rust_string(base_url)});
        assert!(SourceConfig::BASE_URL.starts_with("https://"));
        let filters: serde_json::Value =
            serde_json::from_str(SourceConfig::FILTERS_JSON).unwrap();
        let filters = filters.to_string().to_lowercase();
        for blocked in [
            "18+", "21+", "adult", "ecchi", "hentai", "porn", "sex", "smut",
            "бдсм", "изнасил", "инцест", "порно", "секс", "смат", "хентай",
            "эрот", "этти", "эччи",
        ] {{
            assert!(!filters.contains(blocked), "unsafe filter {{blocked}}");
        }}
    }}
}}
'''


def cargo_toml(source_id: str) -> str:
    return f'''[package]
name = "manatan-novel-ru-{source_id}"
version.workspace = true
edition.workspace = true
license = "MIT"
repository.workspace = true
publish = false

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
manatan-sdk.workspace = true
rulate-novel.workspace = true
serde_json.workspace = true
'''


def manifest(source: dict, source_id: str, base_url: str, has_filters: bool) -> dict:
    parsed = urlparse(base_url)
    origin = f"{parsed.scheme}://{parsed.netloc}"
    return {
        "schemaVersion": 2,
        "id": source_id,
        "name": source["sourceName"],
        "version": "0.1.0",
        "versionCode": 1,
        "apiVersion": 2,
        "wasm": "extension.wasm",
        "contentType": "novel",
        "publisher": {
            "id": "org.manatan.community.extensions",
            "publicKey": PUBLISHER_KEY,
            "signature": "0" * 128,
        },
        "description": (
            f"Russian web novels from {source['sourceName']}; adult and unknown "
            "classifications are blocked fail-closed."
        ),
        "author": "Manatan Community",
        "homepage": base_url,
        "repository": "https://github.com/Manatan-Community/extensions-source",
        "license": "MIT",
        "permissions": {"network": {"allow": [origin]}},
        "assets": [],
        "sources": [
            {
                "id": source_id,
                "name": source["sourceName"],
                "lang": "ru",
                "contentType": "novel",
                "baseUrl": base_url,
                "contentRating": "suggestive",
                "capabilities": {
                    "operations": [
                        "commands.describe",
                        "commands.detail.fetch",
                        "commands.chapter.fetch",
                        "commands.content.fetch",
                    ],
                    "search": True,
                    "latest": True,
                    "filters": has_filters,
                    "preferences": False,
                    "urlResolution": True,
                },
                "listings": [
                    {"id": "popular", "name": "Popular"},
                    {"id": "latest", "name": "Latest"},
                ],
                "urlPatterns": [
                    {"pattern": f"{origin}/book/*", "kind": "item-or-chapter"}
                ],
                "tags": ["rulate-api", "compiled-rust", "adult-filtered"],
            }
        ],
    }


def attribution(source: dict) -> str:
    return f'''# Attribution

The {source["sourceName"]} descriptor and API request contract were adapted
from LNReader's MIT-licensed `rulate` multi-source plugin. Runtime parsing and
all content-policy checks are compiled Rust; the upstream TypeScript is not
packaged, downloaded, or evaluated.
'''


def atomic_leaf(source: dict, source_id: str, base_url: str, filters: dict) -> Path:
    target = ROOT / "novel" / "ru" / source_id
    if target.exists():
        marker = target / "src/lib.rs"
        if not marker.is_file() or GENERATED_MARKER not in marker.read_text():
            raise RuntimeError(f"refusing to overwrite non-generated leaf {target}")
    target.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{source_id}-", dir=target.parent))
    try:
        (stage / "src").mkdir()
        (stage / "src/lib.rs").write_text(source_lib(source, source_id, base_url))
        (stage / "Cargo.toml").write_text(cargo_toml(source_id))
        (stage / "filters.json").write_text(
            json.dumps(filters, ensure_ascii=False, indent=2) + "\n"
        )
        (stage / "manifest.json").write_text(
            json.dumps(
                manifest(source, source_id, base_url, bool(filters.get("filters"))),
                ensure_ascii=False,
                indent=2,
            )
            + "\n"
        )
        (stage / "ATTRIBUTION.md").write_text(attribution(source))
        shutil.copy2(FAMILY.parent.parent.parent / "LICENSE", stage / "LICENSE")
        if target.exists():
            shutil.rmtree(target)
        os.replace(stage, target)
    finally:
        if stage.exists():
            shutil.rmtree(stage)
    return target


def matrix_entry(
    source: dict,
    source_id: str,
    status: str,
    reason: str | None,
) -> dict:
    tests: list[str] = []
    package_path = None
    if status in {"implemented", "component-valid", "runtime-tested", "live-verified"}:
        tests = [
            "cargo-test-shared-rulate-saved-json-fixtures",
            "cargo-test-generated-leaf-identity-and-filter-policy",
            f"xtask-build-component-novel/ru/{source_id}",
        ]
    if status in {"component-valid", "runtime-tested", "live-verified"}:
        tests.append(f"xtask-signed-package-validation-novel/ru/{source_id}")
        package_path = f"packages/novel/ru/{source_id}.manatan2"
    if status in {"runtime-tested", "live-verified"}:
        tests.append(f"production-wasmtime-runtime-filters-novel/ru/{source_id}")
    return {
        "upstreamRepository": "LNReader/lnreader-plugins",
        "upstreamPath": f"plugins/multisrc/rulate/sources.json#{source['id']}",
        "sourceId": source_id,
        "language": "ru",
        "mediaKind": "novel",
        "framework": "rulate-lnreader",
        "requiredCapabilities": [
            "http",
            "filters",
            "url-resolution",
            "commands",
        ],
        "status": status,
        "tests": tests,
        "packagePath": package_path,
        "knownSiteFailure": reason,
        "license": "MIT",
        "attribution": (
            "Generated from the MIT-licensed LNReader Rulate descriptor; "
            "runtime behavior is compiled Rust."
        ),
    }


def update_matrix(entries: list[dict]) -> None:
    path = ROOT / "porting-matrix.json"
    matrix = json.loads(path.read_text())
    for entry in entries:
        matches = [
            item
            for item in matrix["sources"]
            if item.get("sourceId") == entry["sourceId"]
            and item.get("language") == "ru"
            and item.get("mediaKind") == "novel"
        ]
        exact = next(
            (
                item
                for item in matches
                if item.get("upstreamRepository") == entry["upstreamRepository"]
                and item.get("upstreamPath") == entry["upstreamPath"]
            ),
            None,
        )
        existing = exact or (matches[0] if matches else None)
        if (
            existing
            and entry["status"] == "component-valid"
            and existing.get("status") in {"runtime-tested", "live-verified"}
        ):
            # Preserve independently collected signed-runtime/live evidence
            # across idempotent descriptor regeneration.
            entry["status"] = existing["status"]
            entry["tests"] = existing.get("tests", entry["tests"])
            entry["packagePath"] = existing.get("packagePath", entry["packagePath"])
            entry["knownSiteFailure"] = existing.get("knownSiteFailure")
        if existing is None:
            matrix["sources"].append(entry)
        else:
            existing.clear()
            existing.update(entry)
            matrix["sources"] = [
                item for item in matrix["sources"] if item is existing or item not in matches
            ]
    matrix["sources"].sort(
        key=lambda item: (
            item.get("mediaKind", ""),
            item.get("language", ""),
            item.get("sourceId", ""),
            item.get("upstreamPath", ""),
        )
    )
    path.write_text(json.dumps(matrix, ensure_ascii=False, indent=2) + "\n")


def main() -> None:
    sources = json.loads((FAMILY / "sources.json").read_text())
    report: dict[str, object] = {
        "family": "rulate",
        "probeDate": "2026-07-19",
        "generated": [],
        "blocked": [],
        "notes": [
            "Rulate returned valid API JSON during the fresh probe, then intermittently presented DDoS-Guard after repeated requests.",
            "Bllate search, detail, chapter-list, and chapter-text endpoints returned valid API JSON during the fresh probe.",
        ],
        "liveVerification": {
            "bllate-api": (
                "The signed package completed live catalog, fail-closed detail, "
                "free chapters, chapter text, and cover bytes through Manatan's "
                "production ExtensionRunner on 2026-07-19."
            ),
            "rulate-api": (
                "The signed package imported and executed offline/runtime filters, "
                "but the live catalog request was blocked by HTTP 403 from server "
                "ddos-guard on 2026-07-19 after earlier successful API probes."
            ),
        },
    }
    entries = []
    for source in sources:
        source_id = canonical_id(source["id"])
        reason = BLOCKED.get(source["id"])
        if reason:
            report["blocked"].append({"id": source_id, "reason": reason})
            entries.append(matrix_entry(source, source_id, "blocked-upstream", reason))
            continue

        base_url = source["sourceSite"].rstrip("/")
        parsed = urlparse(base_url)
        if parsed.scheme != "https" or not parsed.netloc or parsed.path not in {"", "/"}:
            raise RuntimeError(f"source {source_id} is not an exact HTTPS origin")
        filters = sanitized_filters(source)
        target = atomic_leaf(source, source_id, base_url, filters)
        package = ROOT / "dist/packages/novel/ru" / f"{source_id}.manatan2"
        status = "component-valid" if package.is_file() else "implemented"
        report["generated"].append(
            {
                "id": source_id,
                "path": str(target.relative_to(ROOT)),
                "baseUrl": base_url,
                "safeGenreCount": len(
                    filters.get("filters", {}).get("genres", {}).get("options", [])
                ),
            }
        )
        entries.append(matrix_entry(source, source_id, status, None))

    update_matrix(entries)
    report_path = ROOT / "generated/lnreader-rulate.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(
        json.dumps(
            {
                "generated": len(report["generated"]),
                "blocked": len(report["blocked"]),
            }
        )
    )


if __name__ == "__main__":
    main()
