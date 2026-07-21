#!/usr/bin/env python3
"""Generate Play-compatible compiled iFreedom-family `.manatan2` leaves.

Only sanitized metadata and Rust/Wasm are packaged. LNReader's TypeScript is
used as an MIT-licensed porting reference and is never shipped or evaluated.
"""

from __future__ import annotations

import copy
import json
import os
import shutil
import tempfile
from pathlib import Path
from urllib.parse import urlparse

from lnreader_paths import plugins_checkout


ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = plugins_checkout()
FAMILY = UPSTREAM / "plugins/multisrc/ifreedom"
PUBLISHER_KEY = "88b67d201d387960b96b64b5c4ca39d5edceef6e8a088316449a2d5437a889ac"
GENERATED_MARKER = "Generated from LNReader's MIT-licensed iFreedom-family descriptor."
LAYOUTS = {"ifreedom": "Modern", "bookhamster": "Classic"}
BLOCKED_TERMS = (
    "18+", "21+", "adult", "bdsm", "ecchi", "erotic", "erotica",
    "explicit", "hentai", "incest", "mature", "netorare", "porn",
    "rape", "sex", "smut", "аналь", "бдсм", "инцест", "изнасил",
    "нетораре", "порно", "секс", "хентай", "эрот", "этти", "эччи",
)


def blocked(value: str) -> bool:
    normalized = " ".join(value.lower().replace("_", " ").replace("-", " ").split())
    return any(term in normalized for term in BLOCKED_TERMS)


def sanitized_filters() -> dict:
    payload = copy.deepcopy(json.loads((FAMILY / "settings.json").read_text()))
    for definition in payload.get("filters", {}).values():
        definition["options"] = [
            option
            for option in definition.get("options", [])
            if not blocked(str(option.get("label", "")))
            and not blocked(str(option.get("value", "")))
        ]
        default = definition.get("value")
        if isinstance(default, str) and blocked(default):
            definition["value"] = ""
        elif isinstance(default, list):
            definition["value"] = [item for item in default if not blocked(str(item))]
    return payload


def rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def source_lib(source: dict) -> str:
    source_id = source["id"]
    layout = LAYOUTS[source_id]
    return f'''// {GENERATED_MARKER}
// Runtime behavior is compiled Rust; no source code is downloaded or evaluated.

use ifreedom_novel::{{IfreedomConfig, IfreedomLayout}};

#[derive(Default)]
pub struct SourceConfig;

impl IfreedomConfig for SourceConfig {{
    const NAME: &'static str = {rust_string(source["sourceName"])};
    const BASE_URL: &'static str = {rust_string(source["sourceSite"].rstrip("/"))};
    const LAYOUT: IfreedomLayout = IfreedomLayout::{layout};
    const FILTERS_JSON: &'static str = include_str!("../filters.json");
}}

ifreedom_novel::export_ifreedom_extension!(SourceConfig, {rust_string(source_id)});

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn preserves_exact_compiled_identity_and_safe_filters() {{
        assert_eq!(SourceConfig::NAME, {rust_string(source["sourceName"])});
        assert_eq!(SourceConfig::BASE_URL, {rust_string(source["sourceSite"].rstrip("/"))});
        assert!(SourceConfig::BASE_URL.starts_with("https://"));
        let filters = SourceConfig::FILTERS_JSON.to_lowercase();
        for blocked in [
            "adult", "ecchi", "erotic", "hentai", "mature", "porn", "smut", "эрот", "этти",
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
ifreedom-novel.workspace = true
serde_json.workspace = true
'''


def manifest(source: dict) -> dict:
    source_id = source["id"]
    base_url = source["sourceSite"].rstrip("/")
    origin = f"{urlparse(base_url).scheme}://{urlparse(base_url).netloc}"
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
        "sources": [{
            "id": source_id,
            "name": source["sourceName"],
            "lang": "ru",
            "contentType": "novel",
            "baseUrl": base_url,
            "contentRating": "suggestive",
            "capabilities": {
                "operations": [
                    "commands.describe", "commands.detail.fetch",
                    "commands.chapter.fetch", "commands.content.fetch",
                ],
                "search": True,
                "latest": True,
                "filters": True,
                "preferences": False,
                "urlResolution": True,
            },
            "listings": [
                {"id": "popular", "name": "Popular"},
                {"id": "latest", "name": "Latest"},
            ],
            "urlPatterns": [
                {"pattern": f"{origin}/ranobe/*", "kind": "item"},
                {"pattern": f"{origin}/*/*", "kind": "chapter"},
            ],
            "tags": ["ifreedom-family", "compiled-rust", "adult-filtered"],
        }],
    }


def attribution(source: dict) -> str:
    return f'''# Attribution

The {source["sourceName"]} descriptor and parsing contract were adapted from
LNReader's MIT-licensed `ifreedom` multi-source plugin. Runtime parsing and
content-policy checks are compiled Rust; upstream TypeScript is not packaged,
downloaded, or evaluated.
'''


def atomic_leaf(source: dict, filters: dict) -> Path:
    source_id = source["id"]
    target = ROOT / "novel" / "ru" / source_id
    if target.exists():
        marker = target / "src/lib.rs"
        if not marker.is_file() or GENERATED_MARKER not in marker.read_text():
            raise RuntimeError(f"refusing to overwrite non-generated leaf {target}")
    target.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{source_id}-", dir=target.parent))
    try:
        (stage / "src").mkdir()
        (stage / "src/lib.rs").write_text(source_lib(source))
        (stage / "Cargo.toml").write_text(cargo_toml(source_id))
        (stage / "filters.json").write_text(json.dumps(filters, ensure_ascii=False, indent=2) + "\n")
        (stage / "manifest.json").write_text(json.dumps(manifest(source), ensure_ascii=False, indent=2) + "\n")
        (stage / "ATTRIBUTION.md").write_text(attribution(source))
        shutil.copy2(FAMILY.parent.parent.parent / "LICENSE", stage / "LICENSE")
        if target.exists():
            shutil.rmtree(target)
        os.replace(stage, target)
    finally:
        if stage.exists():
            shutil.rmtree(stage)
    return target


def matrix_entry(source: dict, status: str) -> dict:
    source_id = source["id"]
    tests = [
        "cargo-test-shared-ifreedom-saved-html-fixtures",
        "cargo-test-generated-leaf-identity-and-filter-policy",
        f"xtask-build-component-novel/ru/{source_id}",
    ]
    package_path = None
    if status in {"component-valid", "runtime-tested", "live-verified"}:
        tests.append(f"xtask-signed-package-validation-novel/ru/{source_id}")
        package_path = f"packages/novel/ru/{source_id}.manatan2"
    if status in {"runtime-tested", "live-verified"}:
        tests.append(f"production-wasmtime-runtime-novel/ru/{source_id}")
    return {
        "upstreamRepository": "LNReader/lnreader-plugins",
        "upstreamPath": f"plugins/multisrc/ifreedom/sources.json#{source_id}",
        "sourceId": source_id,
        "language": "ru",
        "mediaKind": "novel",
        "framework": "ifreedom-lnreader",
        "requiredCapabilities": ["http", "filters", "url-resolution", "commands"],
        "status": status,
        "tests": tests,
        "packagePath": package_path,
        "knownSiteFailure": None,
        "license": "MIT",
        "attribution": (
            "Generated from the MIT-licensed LNReader iFreedom descriptor; "
            "runtime behavior is compiled Rust."
        ),
    }


def update_matrix(entries: list[dict]) -> None:
    path = ROOT / "porting-matrix.json"
    matrix = json.loads(path.read_text())
    for entry in entries:
        matches = [
            item for item in matrix["sources"]
            if item.get("sourceId") == entry["sourceId"]
            and item.get("language") == "ru"
            and item.get("mediaKind") == "novel"
        ]
        exact = next((
            item for item in matches
            if item.get("upstreamRepository") == entry["upstreamRepository"]
            and item.get("upstreamPath") == entry["upstreamPath"]
        ), None)
        existing = exact or (matches[0] if matches else None)
        if existing and entry["status"] == "component-valid" and existing.get("status") in {"runtime-tested", "live-verified"}:
            entry["status"] = existing["status"]
            entry["tests"] = existing.get("tests", entry["tests"])
            entry["packagePath"] = existing.get("packagePath", entry["packagePath"])
            entry["knownSiteFailure"] = existing.get("knownSiteFailure")
        if existing is None:
            matrix["sources"].append(entry)
        else:
            existing.clear()
            existing.update(entry)
            matrix["sources"] = [item for item in matrix["sources"] if item is existing or item not in matches]
    matrix["sources"].sort(key=lambda item: (
        item.get("mediaKind", ""), item.get("language", ""),
        item.get("sourceId", ""), item.get("upstreamPath", ""),
    ))
    path.write_text(json.dumps(matrix, ensure_ascii=False, indent=2) + "\n")


def main() -> None:
    sources = json.loads((FAMILY / "sources.json").read_text())
    filters = sanitized_filters()
    report = {
        "family": "ifreedom",
        "probeDate": "2026-07-19",
        "generated": [],
        "blocked": [],
        "notes": [
            "Both exact HTTPS origins returned live catalog and detail HTML during the fresh probe.",
            "Catalog entries are withheld until their detail genres are fetched and classified.",
            "Unsafe filter options and unsafe/VIP chapter rows are removed; missing genre metadata fails closed.",
        ],
        "liveVerification": {
            "ifreedom": (
                "The signed package completed live classified catalog/search, "
                "revalidated detail, cover bytes, 2,994 safe chapters, and "
                "first/middle/last chapter text through Manatan's production "
                "ExtensionRunner on 2026-07-19."
            ),
            "bookhamster": (
                "The signed package completed live classified catalog/search, "
                "revalidated detail, cover bytes, 1,393 safe chapters, and "
                "first/middle/last chapter text through Manatan's production "
                "ExtensionRunner on 2026-07-19."
            ),
        },
    }
    entries = []
    safe_genres = len(filters.get("filters", {}).get("genre", {}).get("options", []))
    for source in sources:
        base_url = source["sourceSite"].rstrip("/")
        parsed = urlparse(base_url)
        if parsed.scheme != "https" or not parsed.netloc or parsed.path not in {"", "/"}:
            raise RuntimeError(f"source {source['id']} is not an exact HTTPS origin")
        target = atomic_leaf(source, filters)
        package = ROOT / "dist/packages/novel/ru" / f"{source['id']}.manatan2"
        status = "component-valid" if package.is_file() else "implemented"
        report["generated"].append({
            "id": source["id"],
            "path": str(target.relative_to(ROOT)),
            "baseUrl": base_url,
            "layout": LAYOUTS[source["id"]],
            "safeGenreCount": safe_genres,
        })
        entries.append(matrix_entry(source, status))
    update_matrix(entries)
    report_path = ROOT / "generated/lnreader-ifreedom.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"generated": len(report["generated"]), "safeGenres": safe_genres}))


if __name__ == "__main__":
    main()
