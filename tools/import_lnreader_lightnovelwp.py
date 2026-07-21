#!/usr/bin/env python3
"""Generate compiled-Rust LightNovelWP leaves from LNReader descriptors.

This is a one-way authoring tool. Generated extensions embed sanitized filter
metadata and call the shared Rust family implementation; they do not package
or evaluate the upstream TypeScript/custom JavaScript at runtime.
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from urllib.parse import urlparse

from lnreader_paths import plugins_checkout


ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = plugins_checkout()
FAMILY = UPSTREAM / "plugins/multisrc/lightnovelwp"
PUBLISHER_KEY = "88b67d201d387960b96b64b5c4ca39d5edceef6e8a088316449a2d5437a889ac"

LANGUAGES = {
    "English": "en",
    "Arabic": "ar",
    "French": "fr",
    "Indonesian": "id",
    "Portuguese": "pt",
    "Spanish": "es",
    "Turkish": "tr",
}

# Fresh probes on 2026-07-19 showed these unflagged descriptors no longer
# expose their claimed LightNovelWP source. They remain explicit matrix rows
# rather than becoming packages that appear installed but cannot work.
FRESH_BLOCKS = {
    "freekolnovel": "The source now redirects to kolnovel.com and no longer exposes an independent Free Kol Novel catalog (verified 2026-07-19).",
    "knoxt": "The catalog endpoint returns a fixed HTTP 403 response with no LightNovelWP catalog (verified 2026-07-19).",
    "universalnovel": "The catalog endpoint returns a fixed HTTP 403 response with no LightNovelWP catalog (verified 2026-07-19).",
    "noveltr": "The domain now serves an unrelated travel website and /series/ returns 404 (verified 2026-07-19).",
    "noblemtl": "The domain now serves a text romanization tool rather than a novel catalog (verified 2026-07-19).",
    "novelsknight": "The source host returns Cloudflare 521 Web server is down (verified 2026-07-19).",
    "whitemoonlightnovels": "The source redirects to a parked advertising endpoint and has no novel catalog (verified 2026-07-19).",
    "systemtranslation": "The source has an expired TLS certificate and cannot be reached through the HTTPS-only runtime (verified 2026-07-19).",
    "lightnovelbrasil": "The source resolves to a parked loading page and has no novel catalog (verified 2026-07-19).",
    "ippotranslations": "The source explicitly says it moved and no longer exposes a catalog; the replacement is a separate source (verified 2026-07-19).",
    "vandytranslate": "The domain now serves an unrelated games/FAQ site and has no LightNovelWP catalog (verified 2026-07-19).",
}

BASE_OVERRIDES = {
    "lightnovelfr": "https://novel-fr.net/",
}

TRANSFORMS = {
    "kolnovel": "ChapterTransform::KolNovel",
    "freekolnovel": "ChapterTransform::KolNovel",
    "novelsknight": "ChapterTransform::NovelsKnight",
    "requiemtls": "ChapterTransform::Requiem",
}

BLOCKED_TERMS = (
    "adult",
    "ecchi",
    "etchi",
    "erotic",
    "erotica",
    "explicit",
    "hentai",
    "mature",
    "porn",
    "smut",
    "adulta",
    "adulte",
    "adulto",
    "dewasa",
    "erotik",
    "erotis",
    "erótico",
    "erótica",
    "érotique",
    "madura",
    "maduro",
    "yetişkin",
    "إيتشي",
    "ناضج",
    "بالغ",
    "للبالغين",
    "إباحي",
)


def canonical_id(value: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")


def rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def blocked_option(value: str) -> bool:
    normalized = re.sub(r"[_-]+", " ", value.lower())
    normalized = " ".join(normalized.split())
    # Exact-only aliases used by some catalogs for their adult genre. Avoid a
    # broad `18` substring match because that would reject unrelated values.
    if normalized in {
        "18", "+18", "18+", "r18", "r 18",
        "١٨", "+١٨", "١٨+", "sm",
    }:
        return True
    return any(
        term.replace("-", " ") in normalized
        for term in BLOCKED_TERMS
    )


def sanitized_filters(source_id: str) -> dict:
    path = FAMILY / "filters" / f"{source_id}.json"
    if not path.exists():
        return {"filters": {}}
    payload = json.loads(path.read_text())
    for definition in payload.get("filters", {}).values():
        definition["options"] = [
            option
            for option in definition.get("options", [])
            if not blocked_option(str(option.get("label", "")))
            and not blocked_option(str(option.get("value", "")))
        ]
    return payload


def source_lib(source: dict, source_id: str, base_url: str, lang: str) -> str:
    options = source.get("options", {})
    series_path = options.get("seriesPath", "/series/").strip("/") + "/"
    transform = TRANSFORMS.get(source["id"])
    import_line = (
        "use lightnovelwp_novel::{ChapterTransform, LightNovelWpConfig};"
        if transform
        else "use lightnovelwp_novel::LightNovelWpConfig;"
    )
    constants = []
    if series_path != "series/":
        constants.append(f"    const SERIES_PATH: &'static str = {rust_string(series_path)};")
    if not options.get("reverseChapters", False):
        constants.append("    const REVERSE_CHAPTERS: bool = false;")
    if options.get("hasLocked"):
        constants.append("    const HAS_LOCKED_CHAPTERS: bool = true;")
    if transform:
        constants.append(f"    const CHAPTER_TRANSFORM: ChapterTransform = {transform};")
    joined = "\n".join(constants)
    extra_constants = f"\n{joined}" if joined else ""
    assertions = []
    if transform:
        assertions.append(f"        assert_eq!(SourceConfig::CHAPTER_TRANSFORM, {transform});")
    if options.get("hasLocked"):
        assertions.extend([
            "        use manatan_sdk::NovelSource;",
            "        assert_eq!(Source::default().preferences().unwrap().len(), 1);",
        ])
    extra_assertions = f"\n{chr(10).join(assertions)}" if assertions else ""
    return f'''// Generated from an MIT-licensed third-party LightNovelWP descriptor.
// Runtime behavior and site-specific transforms are compiled Rust.

{import_line}

#[derive(Default)]
pub struct SourceConfig;

impl LightNovelWpConfig for SourceConfig {{
    const NAME: &'static str = {rust_string(source["sourceName"])};
    const BASE_URL: &'static str = {rust_string(base_url.rstrip("/"))};
    const LANG: &'static str = {rust_string(lang)};
    const FILTERS_JSON: &'static str = include_str!("../filters.json");{extra_constants}
}}

lightnovelwp_novel::export_lightnovelwp_extension!(SourceConfig, {rust_string(source_id)});

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn preserves_compiled_descriptor_identity() {{
        assert_eq!(SourceConfig::NAME, {rust_string(source["sourceName"])});
        assert_eq!(SourceConfig::BASE_URL, {rust_string(base_url.rstrip("/"))});
        assert_eq!(SourceConfig::LANG, {rust_string(lang)});
        assert!(SourceConfig::BASE_URL.starts_with("https://"));
        let filters: serde_json::Value = serde_json::from_str(SourceConfig::FILTERS_JSON).unwrap();
        let serialized = filters.to_string().to_lowercase();
        for blocked in [
            "adult", "ecchi", "hentai", "mature", "smut", "+18", "\\\"18\\\"", "\\\"sm\\\"",
        ] {{
            assert!(!serialized.contains(blocked), "unsafe filter {{blocked}}");
        }}{extra_assertions}
    }}
}}
'''


def manifest(source: dict, source_id: str, base_url: str, lang: str, has_filters: bool) -> dict:
    parsed = urlparse(base_url)
    origin = f"{parsed.scheme}://{parsed.netloc}"
    capabilities = {
        "operations": [
            "commands.describe",
            "commands.detail.fetch",
            "commands.chapter.fetch",
            "commands.content.fetch",
        ],
        "search": True,
        "latest": True,
        "filters": has_filters,
        "preferences": bool(source.get("options", {}).get("hasLocked")),
        "urlResolution": True,
    }
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
        "homepage": base_url,
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
                "baseUrl": base_url.rstrip("/"),
                "contentRating": "suggestive",
                "capabilities": capabilities,
                "listings": [
                    {"id": "popular", "name": "Popular"},
                    {"id": "latest", "name": "Latest"},
                ],
                "urlPatterns": [
                    {"pattern": f"{origin}/*", "kind": "item-or-chapter"}
                ],
                "tags": ["lightnovelwp", "compiled-rust", "adult-filtered"],
            }
        ],
    }


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
lightnovelwp-novel.workspace = true
manatan-sdk.workspace = true
serde_json.workspace = true
'''


def matrix_entry(source: dict, source_id: str, lang: str, status: str, reason: str | None) -> dict:
    tests = []
    package_path = None
    if status in {"implemented", "component-valid", "runtime-tested", "live-verified"}:
        tests = [
            "cargo-test-shared-lightnovelwp-saved-fixtures",
            "cargo-test-generated-leaf-identity-and-filter-policy",
            f"xtask-build-component-novel/{lang}/{source_id}",
        ]
    if status in {"component-valid", "runtime-tested", "live-verified"}:
        tests.append(f"xtask-signed-package-validation-novel/{lang}/{source_id}")
        package_path = f"packages/novel/{lang}/{source_id}.manatan2"
    if status in {"runtime-tested", "live-verified"}:
        tests.append(f"production-wasmtime-runtime-filters-novel/{lang}/{source_id}")
    return {
        "upstreamRepository": "LNReader/lnreader-plugins",
        "upstreamPath": f"plugins/multisrc/lightnovelwp/sources.json#{source['id']}",
        "sourceId": source_id,
        "language": lang,
        "mediaKind": "novel",
        "framework": "lightnovelwp-lnreader",
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
        "attribution": "Generated from the MIT-licensed LNReader LightNovelWP descriptor; runtime behavior is compiled Rust.",
    }


def main() -> None:
    sources = json.loads((FAMILY / "sources.json").read_text())
    report = {"family": "lightnovelwp", "generated": [], "blocked": [], "skipped": []}
    matrix_path = ROOT / "porting-matrix.json"
    matrix = json.loads(matrix_path.read_text())

    for source in sources:
        if source["id"] in {"hyacinthbloom", "blumeverse"}:
            report["skipped"].append({"id": source["id"], "reason": "not requested"})
            continue
        lang = LANGUAGES[source.get("options", {}).get("lang", "English")]
        source_id = canonical_id(source["id"])
        reason = None
        if source.get("options", {}).get("down"):
            reason = f"Upstream descriptor is marked down (downSince={source['options'].get('downSince')})."
        elif source["id"] in FRESH_BLOCKS:
            reason = FRESH_BLOCKS[source["id"]]

        if reason:
            status = "blocked-upstream"
            report["blocked"].append({"id": source_id, "reason": reason})
        else:
            package_file = ROOT / "dist/packages/novel" / lang / f"{source_id}.manatan2"
            status = "component-valid" if package_file.is_file() else "implemented"
            base_url = BASE_OVERRIDES.get(source["id"], source["sourceSite"])
            filter_payload = sanitized_filters(source["id"])
            target = ROOT / "novel" / lang / source_id
            if target.exists() and "Generated from an MIT-licensed third-party LightNovelWP descriptor" not in (target / "src/lib.rs").read_text():
                raise RuntimeError(f"refusing to overwrite non-generated leaf {target}")
            (target / "src").mkdir(parents=True, exist_ok=True)
            (target / "src/lib.rs").write_text(
                source_lib(source, source_id, base_url, lang)
            )
            (target / "Cargo.toml").write_text(cargo_toml(source_id, lang))
            (target / "filters.json").write_text(
                json.dumps(filter_payload, ensure_ascii=False, indent=2) + "\n"
            )
            (target / "manifest.json").write_text(
                json.dumps(
                    manifest(
                        source,
                        source_id,
                        base_url,
                        lang,
                        bool(filter_payload.get("filters")),
                    ),
                    ensure_ascii=False,
                    indent=2,
                )
                + "\n"
            )
            report["generated"].append(
                {
                    "id": source_id,
                    "path": str(target.relative_to(ROOT)),
                    "baseUrl": base_url,
                }
            )

        upstream_path = f"plugins/multisrc/lightnovelwp/sources.json#{source['id']}"
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
            None,
        )
        if existing is None and duplicates:
            existing = duplicates[0]
        # Runtime evidence remains valid across idempotent regeneration while
        # the signed package still exists. If the package is removed, the
        # earlier implemented/component-valid computation safely downgrades it.
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
                item
                for item in matrix["sources"]
                if item is existing or item not in duplicates
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
    report_path = ROOT / "generated/lnreader-lightnovelwp.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({key: len(value) for key, value in report.items() if isinstance(value, list)}))


if __name__ == "__main__":
    main()
