#!/usr/bin/env python3
"""Generate compiled-Rust ReadWN leaves from selected LNReader descriptors.

This is a one-way authoring tool. It embeds sanitized declarative metadata and
icons only. Generated packages call the reviewed Rust family implementation;
they do not package or evaluate upstream TypeScript, JavaScript, bytecode, or
native code at runtime.
"""

from __future__ import annotations

import hashlib
import json
import re
import shutil
from pathlib import Path
from urllib.parse import urlparse

from lnreader_paths import plugins_checkout


ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = plugins_checkout()
FAMILY = UPSTREAM / "plugins/multisrc/readwn"
ICONS = UPSTREAM / "public/static/multisrc/readwn"
PUBLISHER_KEY = "88b67d201d387960b96b64b5c4ca39d5edceef6e8a088316449a2d5437a889ac"
ACTIVE_IDS = ("wuxiap", "ltnovel", "wuxiamtl", "fannovel", "wuxiaspace")
BLOCKED_IDS = ("wuxiav",)
BLOCKED_TERMS = (
    "adult",
    "mature",
    "smut",
    "ecchi",
    "hentai",
    "erotic",
    "porn",
    "lolicon",
    "shotacon",
    "nsfw",
    "explicit",
    "sexual",
    "nudity",
    "incest",
    "rape",
    "noncon",
    "adultery",
)


def blocked_option(value: str) -> bool:
    normalized = " ".join(re.sub(r"[_-]+", " ", value.lower()).split())
    tokens = set(re.findall(r"[a-z0-9]+", normalized))
    if normalized in {"18", "+18", "18+", "r18", "r 18", "18plus"}:
        return True
    return any(term in tokens or term in normalized for term in BLOCKED_TERMS)


def sanitize_filters(source_id: str) -> dict:
    path = FAMILY / "filters" / f"{source_id}.json"
    if not path.is_file():
        return {"filters": {}}
    payload = json.loads(path.read_text())
    # ReadWN's author/tag picker is an unbounded, site-generated taxonomy. It
    # includes euphemistic and deliberately misspelled sexual classifications
    # that cannot be reviewed safely with a static deny-list. The Play package
    # therefore omits that picker entirely and exposes only the bounded
    # sort/status/genre descriptors.
    payload.get("filters", {}).pop("tags", None)
    for definition in payload.get("filters", {}).values():
        options = [
            option
            for option in definition.get("options", [])
            if not blocked_option(str(option.get("label", "")))
            and not blocked_option(str(option.get("value", "")))
        ]
        definition["options"] = options
        allowed = {str(option.get("value", "")) for option in options}
        if str(definition.get("value", "")) not in allowed:
            definition["value"] = str(options[0].get("value", "")) if options else ""
    return payload


def source_lib(source: dict, filters: dict) -> str:
    source_id = source["id"]
    return f'''// Generated from an MIT-licensed third-party ReadWN descriptor.
// Runtime behavior is reviewed, compiled Rust/WebAssembly.

use readwn_novel::ReadWnConfig;

#[derive(Default)]
pub struct SourceConfig;

impl ReadWnConfig for SourceConfig {{
    const NAME: &'static str = {json.dumps(source["sourceName"])};
    const BASE_URL: &'static str = {json.dumps(source["sourceSite"].rstrip("/"))};
    const FILTERS_JSON: &'static str = include_str!("../filters.json");
}}

readwn_novel::export_readwn_extension!(SourceConfig, {json.dumps(source_id)});

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn preserves_compiled_identity_and_policy() {{
        assert_eq!(SourceConfig::NAME, {json.dumps(source["sourceName"])});
        assert_eq!(SourceConfig::BASE_URL, {json.dumps(source["sourceSite"].rstrip("/"))});
        assert!(SourceConfig::BASE_URL.starts_with("https://"));
        let filters: serde_json::Value = serde_json::from_str(SourceConfig::FILTERS_JSON).unwrap();
        assert!(filters["filters"].get("tags").is_none());
        let serialized = filters.to_string().to_lowercase();
        for blocked in [
            "adult", "mature", "smut", "ecchi", "hentai", "\\\"r18\\\"", "\\\"18\\\"",
        ] {{
            assert!(
                !serialized.contains(blocked),
                "unsafe compiled filter {{blocked}}"
            );
        }}
    }}
}}
'''


def cargo_toml(source_id: str) -> str:
    return f'''[package]
name = "manatan-novel-en-{source_id}"
version.workspace = true
edition.workspace = true
license = "MIT"
repository.workspace = true
publish = false

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
manatan-sdk.workspace = true
readwn-novel.workspace = true
serde_json.workspace = true
'''


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def previous_signature(target: Path) -> str:
    manifest_path = target / "manifest.json"
    if manifest_path.is_file():
        try:
            value = json.loads(manifest_path.read_text())["publisher"]["signature"]
            if re.fullmatch(r"[0-9a-f]{128}", value):
                return value
        except (KeyError, TypeError, ValueError, json.JSONDecodeError):
            pass
    return "0" * 128


def manifest(source: dict, target: Path, filters: dict) -> dict:
    base_url = source["sourceSite"].rstrip("/")
    parsed = urlparse(base_url)
    if parsed.scheme != "https" or not parsed.netloc or parsed.path not in {"", "/"}:
        raise ValueError(f"ReadWN descriptor must use an exact HTTPS origin: {base_url}")
    origin = f"https://{parsed.netloc}"
    icon_path = target / "assets/icon.png"
    return {
        "schemaVersion": 2,
        "id": source["id"],
        "name": source["sourceName"],
        "version": "0.1.0",
        "versionCode": 1,
        "apiVersion": 2,
        "wasm": "extension.wasm",
        "contentType": "novel",
        "publisher": {
            "id": "org.manatan.community.extensions",
            "publicKey": PUBLISHER_KEY,
            "signature": previous_signature(target),
        },
        "description": (
            f"English web novels from {source['sourceName']}; restricted classifications "
            "are removed and content is revalidated before chapters or text are exposed."
        ),
        "author": "Manatan Community",
        "homepage": base_url,
        "repository": "https://github.com/Manatan-Community/extensions-source",
        "license": "MIT",
        "icon": "assets/icon.png",
        "permissions": {"network": {"allow": [origin]}},
        "assets": [
            {
                "path": "assets/icon.png",
                "mimeType": "image/png",
                "sha256": sha256(icon_path),
            }
        ],
        "sources": [
            {
                "id": source["id"],
                "name": source["sourceName"],
                "lang": "en",
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
                    "filters": bool(filters.get("filters")),
                    "urlResolution": True,
                },
                "listings": [
                    {"id": "popular", "name": "Popular"},
                    {"id": "latest", "name": "Latest"},
                ],
                "urlPatterns": [{"pattern": f"{origin}/*", "kind": "item-or-chapter"}],
                "tags": ["readwn", "compiled-rust", "adult-filtered"],
            }
        ],
    }


def attribution(source: dict) -> str:
    return f'''# Attribution

Source identity, origin, filter metadata, and icon are adapted from the
MIT-licensed LNReader ReadWN descriptor `{source["id"]}`.

The upstream executable TypeScript is not packaged, downloaded, or evaluated.
Runtime parsing and policy enforcement are implemented in reviewed Rust and
compiled to WebAssembly by this repository.
'''


def matrix_entry(source: dict, status: str, reason: str | None) -> dict:
    source_id = source["id"]
    tests: list[str] = []
    package_path = None
    if status in {"implemented", "component-valid", "runtime-tested", "live-verified"}:
        tests = [
            "cargo-test-shared-readwn-saved-fixtures",
            "cargo-test-generated-readwn-leaf-identity-and-filter-policy",
            f"xtask-build-component-novel/en/{source_id}",
        ]
    if status in {"component-valid", "runtime-tested", "live-verified"}:
        tests.append(f"xtask-signed-package-validation-novel/en/{source_id}")
        package_path = f"packages/novel/en/{source_id}.manatan2"
    if status in {"runtime-tested", "live-verified"}:
        tests.append(f"production-wasmtime-runtime-smoke-novel/en/{source_id}")
    if status == "live-verified":
        tests.append("fresh-live-readwn-catalog-thumbnail-details-chapters-text")
    return {
        "upstreamRepository": "LNReader/lnreader-plugins",
        "upstreamPath": f"plugins/multisrc/readwn/sources.json#{source_id}",
        "sourceId": source_id,
        "language": "en",
        "mediaKind": "novel",
        "framework": "readwn-lnreader",
        "requiredCapabilities": [
            "http",
            "filters",
            "url-resolution",
            "chapter-pagination",
            "commands",
        ],
        "status": status,
        "tests": tests,
        "packagePath": package_path,
        "knownSiteFailure": reason,
        "license": "MIT",
        "attribution": (
            "Generated from the MIT-licensed LNReader ReadWN descriptor; "
            "runtime behavior is reviewed, compiled Rust."
        ),
    }


def update_matrix(matrix: dict, source: dict, status: str, reason: str | None) -> None:
    upstream_path = f"plugins/multisrc/readwn/sources.json#{source['id']}"
    existing = next(
        (
            row
            for row in matrix["sources"]
            if row.get("upstreamRepository") == "LNReader/lnreader-plugins"
            and row.get("upstreamPath") == upstream_path
            and row.get("sourceId") == source["id"]
            and row.get("language") == "en"
            and row.get("mediaKind") == "novel"
        ),
        None,
    )
    if (
        status == "component-valid"
        and existing is not None
        and existing.get("status") in {"runtime-tested", "live-verified"}
    ):
        status = existing["status"]
        if reason is None:
            reason = existing.get("knownSiteFailure")
    row = matrix_entry(source, status, reason)
    if existing is None:
        matrix["sources"].append(row)
    else:
        existing.clear()
        existing.update(row)


def main() -> None:
    sources = {source["id"]: source for source in json.loads((FAMILY / "sources.json").read_text())}
    matrix_path = ROOT / "porting-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    report = {"family": "readwn", "generated": [], "blocked": [], "skipped": []}

    for source_id in ACTIVE_IDS:
        source = sources[source_id]
        if source.get("options", {}).get("down"):
            raise RuntimeError(f"active ReadWN source is unexpectedly marked down: {source_id}")
        filters = sanitize_filters(source_id)
        target = ROOT / "novel/en" / source_id
        existing_source = target / "src/lib.rs"
        if target.exists() and existing_source.is_file() and "third-party ReadWN descriptor" not in existing_source.read_text():
            raise RuntimeError(f"refusing to overwrite non-generated leaf {target}")
        (target / "src").mkdir(parents=True, exist_ok=True)
        (target / "assets").mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ICONS / source_id / "icon.png", target / "assets/icon.png")
        (target / "src/lib.rs").write_text(source_lib(source, filters))
        (target / "Cargo.toml").write_text(cargo_toml(source_id))
        (target / "filters.json").write_text(json.dumps(filters, ensure_ascii=False, indent=2) + "\n")
        (target / "LICENSE").write_text((FAMILY / "LICENSE").read_text() if (FAMILY / "LICENSE").is_file() else (UPSTREAM / "LICENSE").read_text())
        (target / "ATTRIBUTION.md").write_text(attribution(source))
        (target / "manifest.json").write_text(json.dumps(manifest(source, target, filters), ensure_ascii=False, indent=2) + "\n")

        package = ROOT / "dist/packages/novel/en" / f"{source_id}.manatan2"
        status = "component-valid" if package.is_file() else "implemented"
        update_matrix(matrix, source, status, None)
        report["generated"].append(
            {
                "id": source_id,
                "path": str(target.relative_to(ROOT)),
                "baseUrl": source["sourceSite"].rstrip("/"),
                "filterCount": len(filters.get("filters", {})),
            }
        )

    for source_id in BLOCKED_IDS:
        source = sources[source_id]
        options = source.get("options", {})
        reason = f"Upstream descriptor is marked down (downSince={options.get('downSince')})."
        update_matrix(matrix, source, "blocked-upstream", reason)
        report["blocked"].append({"id": source_id, "reason": reason})

    matrix_path.write_text(json.dumps(matrix, ensure_ascii=False, indent=2) + "\n")
    report_path = ROOT / "generated/lnreader-readwn.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({key: len(value) for key, value in report.items() if isinstance(value, list)}))


if __name__ == "__main__":
    main()
