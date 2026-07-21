#!/usr/bin/env python3
"""Generate compiled-Rust Madara leaves from an LNReader plugin checkout.

The generated packages contain no TypeScript or downloaded code. The upstream
descriptor supplies only source identity, origin, language, and availability;
all runtime behavior lives in the reviewed `shared/madara-novel` Rust crate.
Existing leaves are never overwritten so maintainers can add typed overrides.
"""

from __future__ import annotations

import argparse
import json
import re
import unicodedata
from pathlib import Path


PUBLISHER_ID = "org.manatan.community.extensions"
PUBLISHER_PUBLIC_KEY = (
    "88b67d201d387960b96b64b5c4ca39d5edceef6e8a088316449a2d5437a889ac"
)
ZERO_SIGNATURE = "0" * 128
LANGUAGES = {
    "English": "en",
    "Arabic": "ar",
    "Indonesian": "id",
    "Spanish": "es",
    "Thai": "th",
    "French": "fr",
    "Portuguese": "pt",
    "Korean": "ko",
    "Turkish": "tr",
}


def slug(value: str) -> str:
    normalized = unicodedata.normalize("NFKD", value).encode("ascii", "ignore").decode()
    normalized = normalized.lower().replace("&", " and ")
    normalized = re.sub(r"[^a-z0-9.]+", "-", normalized).strip("-.")
    normalized = re.sub(r"[-.]{2,}", "-", normalized)
    if not normalized or not normalized[0].isalpha():
        normalized = f"source-{normalized}"
    return normalized


def rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def cargo_name(language: str, source_id: str) -> str:
    return f"manatan-novel-{language}-{source_id}".replace(".", "-")


def source_code(name: str, source_id: str, site: str, language: str) -> str:
    return f'''// Generated from an MIT-licensed third-party Madara descriptor.
// Runtime behavior is compiled Rust from shared/madara-novel.

use madara_novel::{{MadaraNovelConfig, MadaraNovelSource}};
#[cfg(target_arch = "wasm32")]
use manatan_sdk::Extension;

#[derive(Default)]
pub struct SourceConfig;

impl MadaraNovelConfig for SourceConfig {{
    const NAME: &'static str = {rust_string(name)};
    const BASE_URL: &'static str = {rust_string(site.rstrip('/'))};
    const LANG: &'static str = {rust_string(language)};
}}

pub type Source = MadaraNovelSource<SourceConfig>;

#[cfg(target_arch = "wasm32")]
fn extension() -> Extension {{
    Extension::new().novel({rust_string(source_id)}, Source::default())
}}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn preserves_descriptor_identity() {{
        assert_eq!(SourceConfig::NAME, {rust_string(name)});
        assert_eq!(SourceConfig::BASE_URL, {rust_string(site.rstrip('/'))});
        assert_eq!(SourceConfig::LANG, {rust_string(language)});
    }}
}}
'''


def manifest(name: str, source_id: str, site: str, language: str) -> dict:
    origin = site.rstrip("/")
    return {
        "schemaVersion": 2,
        "id": source_id,
        "name": name,
        "version": "0.1.0",
        "versionCode": 1,
        "apiVersion": 2,
        "wasm": "extension.wasm",
        "contentType": "novel",
        "publisher": {
            "id": PUBLISHER_ID,
            "publicKey": PUBLISHER_PUBLIC_KEY,
            "signature": ZERO_SIGNATURE,
        },
        "description": f"Web novels from {name}, with mature categories removed.",
        "author": "Manatan Community",
        "homepage": site,
        "repository": "https://github.com/Manatan-Community/extensions-source",
        "license": "MIT",
        "permissions": {"network": {"allow": [origin]}},
        "assets": [],
        "sources": [
            {
                "id": source_id,
                "name": name,
                "lang": language,
                "contentType": "novel",
                "baseUrl": origin,
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
                    "urlResolution": True,
                },
                "listings": [
                    {"id": "popular", "name": "Popular"},
                    {"id": "latest", "name": "Latest"},
                ],
                "urlPatterns": [{"pattern": f"{origin}/*", "kind": "item-or-chapter"}],
                "tags": ["madara", "compiled-rust", "adult-filtered"],
            }
        ],
    }


def matrix_row(source: dict, source_id: str, language: str, down: bool) -> dict:
    reason = "Marked unavailable by the upstream source descriptor." if down else None
    return {
        "upstreamRepository": "LNReader/lnreader-plugins",
        "upstreamPath": f"plugins/multisrc/madara/sources.json#{source['id']}",
        "sourceId": source_id,
        "language": language,
        "mediaKind": "novel",
        "framework": "madara-lnreader",
        "requiredCapabilities": ["http", "filters", "url-resolution", "chapter-pagination"],
        "status": "blocked-upstream" if down else "implemented",
        "tests": [] if down else ["generated-leaf-metadata"],
        "packagePath": None,
        "knownSiteFailure": reason,
        "license": "MIT",
        "attribution": "Generated from the MIT-licensed third-party Madara descriptor.",
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("upstream", type=Path)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()

    source_file = args.upstream / "plugins/multisrc/madara/sources.json"
    sources = json.loads(source_file.read_text())
    matrix_path = args.root / "porting-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    rows = {
        (row["mediaKind"], row["language"], row["sourceId"]): row
        for row in matrix["sources"]
    }
    generated = []
    skipped_existing = []
    blocked = []
    ids = set()

    for source in sources:
        options = source.get("options") or {}
        language_name = options.get("lang", "English")
        if language_name not in LANGUAGES:
            raise SystemExit(f"unsupported Madara language {language_name!r}")
        language = LANGUAGES[language_name]
        source_id = slug(source["sourceName"])
        if source_id in ids:
            raise SystemExit(f"duplicate normalized id {source_id}")
        ids.add(source_id)
        down = bool(options.get("down"))
        key = ("novel", language, source_id)
        destination = args.root / "novel" / language / source_id
        if down:
            rows.setdefault(key, matrix_row(source, source_id, language, down))
            blocked.append(source_id)
            continue

        if destination.exists():
            skipped_existing.append(source_id)
            continue
        rows[key] = matrix_row(source, source_id, language, down)
        (destination / "src").mkdir(parents=True)
        (destination / "Cargo.toml").write_text(
            f'''[package]
name = "{cargo_name(language, source_id)}"
version.workspace = true
edition.workspace = true
license = "MIT"
repository.workspace = true
publish = false

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
madara-novel.workspace = true
manatan-sdk.workspace = true
'''
        )
        (destination / "src/lib.rs").write_text(
            source_code(source["sourceName"], source_id, source["sourceSite"], language)
        )
        (destination / "manifest.json").write_text(
            json.dumps(
                manifest(source["sourceName"], source_id, source["sourceSite"], language),
                ensure_ascii=False,
                indent=2,
            )
            + "\n"
        )
        generated.append(source_id)

    matrix["sources"] = sorted(
        rows.values(),
        key=lambda row: (row["mediaKind"], row["language"], row["sourceId"]),
    )
    matrix_path.write_text(json.dumps(matrix, ensure_ascii=False, indent=2) + "\n")
    report = {
        "family": "madara",
        "upstream": "LNReader/lnreader-plugins",
        "upstreamPath": "plugins/multisrc/madara/sources.json",
        "generated": generated,
        "skippedExisting": skipped_existing,
        "blockedUpstream": blocked,
    }
    report_path = args.root / "generated" / "lnreader-madara.json"
    report_path.parent.mkdir(exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({key: len(value) for key, value in report.items() if isinstance(value, list)}))


if __name__ == "__main__":
    main()
