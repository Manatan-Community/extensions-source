#!/usr/bin/env python3
"""Generate compiled-Rust MTLNovel leaves and maintain their matrix rows.

The current upstream domains are parked, so this run records explicit blocked
rows. If a source becomes viable, removing its verified block produces an
atomic, Play-compatible Rust leaf; no upstream TypeScript is packaged.
"""

from __future__ import annotations

import json
import re
import shutil
from pathlib import Path
from urllib.parse import urlparse

from lnreader_paths import plugins_checkout


ROOT = Path(__file__).resolve().parents[1]
FAMILY = plugins_checkout() / "plugins/multisrc/mtlnovel"
PUBLISHER_KEY = "88b67d201d387960b96b64b5c4ca39d5edceef6e8a088316449a2d5437a889ac"

LANGUAGES = {
    "English": "en",
    "French": "fr",
    "Spanish": "es",
    "Indonesian": "id",
    "Portuguese": "pt",
    "Russian": "ru",
}

# Fresh HTTPS probes on 2026-07-19. None of these responses exposes the
# catalog described by the upstream parser, so emitting packages would create
# installed-but-nonfunctional sources.
FRESH_BLOCKS = {
    "mtlnovel": "The HTTPS origin serves a Loading page whose script redirects into a parked-domain flow ending at insecure http://ww1.mtlnovels.com (verified 2026-07-19).",
    "mtlnovel-fr": "The HTTPS origin serves a Loading page whose script redirects into a parked-domain flow ending at insecure http://ww1.mtlnovels.com (verified 2026-07-19).",
    "mtlnovel-es": "The HTTPS origin serves a Loading page whose script redirects into a parked-domain flow ending at insecure http://ww1.mtlnovels.com (verified 2026-07-19).",
    "mtlnovel-id": "The HTTPS origin serves a Loading page whose script redirects into a parked-domain flow ending at insecure http://ww1.mtlnovels.com (verified 2026-07-19).",
    "mtlnovel-pt": "The HTTPS origin serves a Loading page whose script redirects into a parked-domain flow ending at insecure http://ww1.mtlnovels.com (verified 2026-07-19).",
    "mtlnovel-ru": "The HTTPS origin serves a Loading page whose script redirects into a parked-domain flow ending at insecure http://ww1.mtlnovels.com (verified 2026-07-19).",
}


def canonical_id(value: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")


def rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def source_lib(source: dict, source_id: str, lang: str) -> str:
    return f'''// Generated from an MIT-licensed third-party MTLNovel descriptor.
// Runtime behavior is compiled Rust; downloaded JavaScript is never evaluated.

use mtlnovel_novel::MtlNovelConfig;

#[derive(Default)]
pub struct SourceConfig;

impl MtlNovelConfig for SourceConfig {{
    const NAME: &'static str = {rust_string(source["sourceName"])};
    const BASE_URL: &'static str = {rust_string(source["sourceSite"].rstrip("/"))};
    const LANG: &'static str = {rust_string(lang)};
}}

mtlnovel_novel::export_mtlnovel_extension!(SourceConfig, {rust_string(source_id)});

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn preserves_compiled_descriptor_identity() {{
        assert_eq!(SourceConfig::NAME, {rust_string(source["sourceName"])});
        assert_eq!(SourceConfig::BASE_URL, {rust_string(source["sourceSite"].rstrip("/"))});
        assert_eq!(SourceConfig::LANG, {rust_string(lang)});
        assert!(SourceConfig::BASE_URL.starts_with("https://"));
    }}
}}
'''


def cargo_toml(source_id: str, lang: str) -> str:
    return f'''[package]
name = "manatan-novel-{lang}-{source_id}"
version.workspace = true
edition.workspace = true
license = "MIT"
repository.workspace = true
publish = false

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
manatan-sdk.workspace = true
mtlnovel-novel.workspace = true
'''


def manifest(source: dict, source_id: str, lang: str) -> dict:
    parsed = urlparse(source["sourceSite"])
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
        "description": f"Web novels from {source['sourceName']}, with restricted categories removed.",
        "author": "Manatan Community",
        "homepage": source["sourceSite"],
        "repository": "https://github.com/Manatan-Community/extensions-source",
        "license": "MIT",
        "permissions": {"network": {"allow": [origin]}},
        "assets": [],
        "sources": [
            {
                "id": source_id,
                "name": source["sourceName"],
                "lang": lang,
                "contentType": "novel",
                "baseUrl": source["sourceSite"].rstrip("/"),
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
                    "filters": True,
                    "preferences": False,
                    "urlResolution": True,
                },
                "listings": [
                    {"id": "popular", "name": "Popular"},
                    {"id": "latest", "name": "Latest"},
                ],
                "urlPatterns": [
                    {"pattern": f"{origin}/*", "kind": "item-or-chapter"}
                ],
                "tags": ["mtlnovel", "compiled-rust", "adult-filtered"],
            }
        ],
    }


def matrix_entry(source: dict, source_id: str, lang: str, status: str, reason: str | None) -> dict:
    tests = ["cargo-test-shared-mtlnovel-saved-fixtures"]
    package_path = None
    if status in {"implemented", "component-valid", "runtime-tested", "live-verified"}:
        tests.extend(
            [
                "cargo-test-generated-mtlnovel-leaf-identity",
                f"xtask-build-component-novel/{lang}/{source_id}",
            ]
        )
    if status in {"component-valid", "runtime-tested", "live-verified"}:
        tests.append(f"xtask-signed-package-validation-novel/{lang}/{source_id}")
        package_path = f"packages/novel/{lang}/{source_id}.manatan2"
    if status in {"runtime-tested", "live-verified"}:
        tests.append(f"production-wasmtime-runtime-filters-novel/{lang}/{source_id}")
    if status == "blocked-upstream":
        tests.append("live-https-origin-probe-2026-07-19")
    return {
        "upstreamRepository": "LNReader/lnreader-plugins",
        "upstreamPath": f"plugins/multisrc/mtlnovel/sources.json#{source['id']}",
        "sourceId": source_id,
        "language": lang,
        "mediaKind": "novel",
        "framework": "mtlnovel-lnreader",
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
        "attribution": "Generated from the MIT-licensed LNReader MTLNovel descriptor; runtime behavior is compiled Rust.",
    }


def write_leaf_atomic(source: dict, source_id: str, lang: str) -> Path:
    target = ROOT / "novel" / lang / source_id
    if target.exists():
        marker = target / "src/lib.rs"
        if not marker.is_file() or "Generated from an MIT-licensed third-party MTLNovel descriptor" not in marker.read_text():
            raise RuntimeError(f"refusing to overwrite non-generated leaf {target}")
    staging = ROOT / "generated/mtlnovel-staging" / lang / source_id
    if staging.exists():
        shutil.rmtree(staging)
    (staging / "src").mkdir(parents=True)
    (staging / "src/lib.rs").write_text(source_lib(source, source_id, lang))
    (staging / "Cargo.toml").write_text(cargo_toml(source_id, lang))
    (staging / "manifest.json").write_text(
        json.dumps(manifest(source, source_id, lang), ensure_ascii=False, indent=2) + "\n"
    )
    if target.exists():
        shutil.rmtree(target)
    target.parent.mkdir(parents=True, exist_ok=True)
    staging.rename(target)
    return target


def main() -> None:
    sources = json.loads((FAMILY / "sources.json").read_text())
    report = {"family": "mtlnovel", "generated": [], "blocked": []}
    matrix_path = ROOT / "porting-matrix.json"
    matrix = json.loads(matrix_path.read_text())

    for source in sources:
        source_id = canonical_id(source["id"])
        lang = LANGUAGES[source["options"]["lang"]]
        reason = FRESH_BLOCKS.get(source["id"])
        package_file = ROOT / "dist/packages/novel" / lang / f"{source_id}.manatan2"
        if reason:
            status = "blocked-upstream"
            report["blocked"].append({"id": source_id, "reason": reason})
        else:
            target = write_leaf_atomic(source, source_id, lang)
            status = "component-valid" if package_file.is_file() else "implemented"
            report["generated"].append(
                {"id": source_id, "path": str(target.relative_to(ROOT)), "baseUrl": source["sourceSite"]}
            )

        upstream_path = f"plugins/multisrc/mtlnovel/sources.json#{source['id']}"
        duplicates = [
            item
            for item in matrix["sources"]
            if item.get("sourceId") == source_id
            and item.get("language") == lang
            and item.get("mediaKind") == "novel"
        ]
        existing = next(
            (
                item
                for item in duplicates
                if item.get("upstreamRepository") == "LNReader/lnreader-plugins"
                and item.get("upstreamPath") == upstream_path
            ),
            duplicates[0] if duplicates else None,
        )
        if (
            status == "component-valid"
            and existing is not None
            and existing.get("status") in {"runtime-tested", "live-verified"}
        ):
            status = existing["status"]
        entry = matrix_entry(source, source_id, lang, status, reason)
        if existing is None:
            matrix["sources"].append(entry)
        else:
            existing.clear()
            existing.update(entry)
            matrix["sources"] = [
                item for item in matrix["sources"] if item is existing or item not in duplicates
            ]

    matrix["sources"].sort(
        key=lambda item: (
            item.get("mediaKind", ""),
            item.get("language", ""),
            item.get("sourceId", ""),
            item.get("upstreamPath", ""),
        )
    )
    matrix_path.write_text(json.dumps(matrix, ensure_ascii=False, indent=2) + "\n")
    report_path = ROOT / "generated/lnreader-mtlnovel.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({key: len(value) for key, value in report.items() if isinstance(value, list)}))


if __name__ == "__main__":
    main()
