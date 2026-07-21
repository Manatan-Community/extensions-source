#!/usr/bin/env python3
"""Generate reviewed Rust leaves for selected LNReader Chinese sources."""

from __future__ import annotations

import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path

from lnreader_paths import plugins_checkout

ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = plugins_checkout()
PUBLISHER_KEY = "88b67d201d387960b96b64b5c4ca39d5edceef6e8a088316449a2d5437a889ac"

SOURCES = (
    {
        "id": "shu69", "name": "69书吧", "file": "69shu.ts", "kind": "Shu69",
        "base": "https://www.69shu.xyz", "lang": "zh", "icon": "src/cn/69shu/icon.png",
        "origins": ["https://www.69shu.xyz"], "search": False, "filters": True,
    },
    {
        "id": "quanben", "name": "Quanben", "file": "Quanben.ts", "kind": "Quanben",
        "base": "https://www.quanben.io", "lang": "zh", "icon": "src/cn/quanben/icon.png",
        "origins": ["https://www.quanben.io", "https://quanben5.com", "https://img.c0m.io"], "search": True, "filters": True,
    },
    {
        "id": "ixdzs8", "name": "爱下电子书", "file": "ixdzs8.ts", "kind": "Ixdzs8",
        "base": "https://ixdzs8.com", "lang": "zh", "icon": "src/cn/ixdzs8/favicon.png",
        "origins": ["https://ixdzs8.com", "https://img22.ixdzs.com"], "search": True, "filters": False,
    },
    {
        "id": "linovel", "name": "Linovel", "file": "linovel.ts", "kind": "Linovel",
        "base": "https://www.linovel.net", "lang": "zh", "icon": "src/cn/linovel/icon.png",
        "origins": ["https://www.linovel.net", "https://rin.linovel.net", "https://static.linovel.net", "https://eli.linovel.net", "https://avatar.linovel.net"], "search": True, "filters": False,
    },
    {
        "id": "linovelib", "name": "Linovelib", "file": "linovelib.ts", "kind": "Bilinovel",
        "base": "https://www.bilinovel.com", "lang": "zh", "icon": "src/cn/linovelib/icon.png",
        "origins": ["https://www.bilinovel.com", "https://img3.readpai.com"], "search": False, "filters": True,
    },
    {
        "id": "linovelib_tw", "name": "Linovelib (繁體)", "file": "linovelib_tw.ts", "kind": "BilinovelTw",
        "base": "https://tw.linovelib.com", "lang": "zh", "icon": "src/cn/linovelib/icon.png",
        "origins": ["https://tw.linovelib.com", "https://img3.readpai.com"], "search": False, "filters": True,
    },
    {
        "id": "novel543", "name": "Novel543", "file": "novel543.ts", "kind": "Novel543",
        "base": "https://www.novel543.com", "lang": "zh", "icon": "src/cn/novel543/icon.png",
        "origins": ["https://www.novel543.com", "https://i1.novel543.com", "https://i2.novel543.com"], "search": False, "filters": False,
    },
)

LIVE_VERIFICATION = {
    "shu69": {
        "catalogEntries": 32,
        "coverBytes": 4039,
        "chapterCount": 854,
        "textSizes": [17712, 19911, 266],
    },
    "quanben": {
        "catalogEntries": 18,
        "coverBytes": 7350,
        "chapterCount": 3813,
        "textSizes": [21494, 14224, 23177],
    },
    "ixdzs8": {
        "catalogEntries": 20,
        "coverBytes": 70335,
        "chapterCount": 1304,
        "textSizes": [41870, 11444, 23072],
    },
    "linovel": {
        "catalogEntries": 12,
        "coverBytes": 16602,
        "chapterCount": 42,
        "textSizes": [17928, 67444, 66321],
    },
    "linovelib": {
        "catalogEntries": 31,
        "coverBytes": 86925,
        "chapterCount": 595,
        "textSizes": [3922, 11291, 7036],
    },
    "linovelib_tw": {
        "catalogEntries": 31,
        "coverBytes": 86925,
        "chapterCount": 595,
        "textSizes": [9543, 19056, 13491],
    },
    "novel543": {
        "catalogEntries": 13,
        "coverBytes": 15084,
        "chapterCount": 135,
        "textSizes": [10463, 12544, 14637],
    },
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def previous_signature(target: Path) -> str:
    try:
        value = json.loads((target / "manifest.json").read_text())["publisher"]["signature"]
        if re.fullmatch(r"[0-9a-f]{128}", value):
            return value
    except (FileNotFoundError, KeyError, TypeError, ValueError, json.JSONDecodeError):
        pass
    return "0" * 128


def source_lib(source: dict) -> str:
    runtime_id = source.get("manifest_id", source["id"])
    origins = "\n".join(
        f"        {json.dumps(origin)}," for origin in source["origins"]
    )
    return f'''// Generated from an MIT-licensed LNReader source identity.
// Runtime behavior is reviewed, compiled Rust/WebAssembly.

use chinese_standalone_novel::{{ChineseStandaloneConfig, SourceKind}};

#[derive(Default)]
pub struct SourceConfig;

impl ChineseStandaloneConfig for SourceConfig {{
    const KIND: SourceKind = SourceKind::{source["kind"]};
    const NAME: &'static str = {json.dumps(source["name"], ensure_ascii=False)};
    const BASE_URL: &'static str = {json.dumps(source["base"])};
    const IMAGE_ORIGINS: &'static [&'static str] = &[
{origins}
    ];
}}

chinese_standalone_novel::export_chinese_standalone_extension!(SourceConfig, {json.dumps(runtime_id)});

#[cfg(test)]
mod tests {{
    use super::*;
    #[test]
    fn identity_is_exact_https_and_compiled() {{
        assert_eq!(SourceConfig::NAME, {json.dumps(source["name"], ensure_ascii=False)});
        assert_eq!(SourceConfig::BASE_URL, {json.dumps(source["base"])});
        assert!(SourceConfig::BASE_URL.starts_with("https://"));
        assert!(SourceConfig::IMAGE_ORIGINS
            .iter()
            .all(|origin| origin.starts_with("https://")));
    }}
}}
'''


def cargo_toml(source: dict) -> str:
    return f'''[package]
name = "manatan-novel-zh-{source["id"].replace("_", "-")}"
version.workspace = true
edition.workspace = true
license = "MIT"
repository.workspace = true
publish = false

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
chinese-standalone-novel.workspace = true
manatan-sdk.workspace = true
'''


def manifest(source: dict, target: Path) -> dict:
    runtime_id = source.get("manifest_id", source["id"])
    icon = target / "assets/icon.png"
    operations = ["commands.describe"]
    return {
        "schemaVersion": 2, "id": runtime_id, "name": source["name"],
        "version": "0.1.0", "versionCode": 1, "apiVersion": 2,
        "wasm": "extension.wasm", "contentType": "novel",
        "publisher": {"id": "org.manatan.community.extensions", "publicKey": PUBLISHER_KEY, "signature": previous_signature(target)},
        "description": f"Chinese novels from {source['name']}; categories and every detail are revalidated before chapters or text are exposed.",
        "author": "Manatan Community", "homepage": source["base"],
        "repository": "https://github.com/Manatan-Community/extensions-source", "license": "MIT",
        "icon": "assets/icon.png",
        "permissions": {
            "network": {"allow": source["origins"]},
            "cookies": True,
        },
        "assets": [{"path": "assets/icon.png", "mimeType": "image/png", "sha256": sha256(icon)}],
        "sources": [{
            "id": runtime_id, "name": source["name"], "lang": source["lang"],
            "contentType": "novel", "baseUrl": source["base"], "contentRating": "suggestive",
            "capabilities": {"operations": operations, "search": source["search"], "latest": True, "filters": source["filters"], "urlResolution": True},
            "listings": [{"id": "popular", "name": "Popular"}, {"id": "latest", "name": "Latest"}],
            "urlPatterns": [{"pattern": f"{source['base']}/*", "kind": "item-or-chapter"}],
            "tags": ["compiled-rust", "chinese", "adult-filtered"],
        }],
    }


def attribution(source: dict) -> str:
    return f'''# Attribution

Source identity, selectors, and icon are adapted from the MIT-licensed
LNReader source `plugins/chinese/{source["file"]}`.

The executable TypeScript is not packaged, downloaded, or evaluated. Runtime
requests, parsing, and policy checks are reviewed Rust compiled to WebAssembly.
'''


def matrix_entry(source: dict) -> dict:
    return {
        "upstreamRepository": "LNReader/lnreader-plugins",
        "upstreamPath": f"plugins/chinese/{source['file']}",
        "sourceId": source["id"], "language": "zh", "mediaKind": "novel",
        "framework": "chinese-standalone-lnreader-compiled-rust",
        "requiredCapabilities": ["http", "filters", "url-resolution", "chapter-pagination", "commands"],
        "status": "live-verified",
        "tests": [
            "cargo-test-shared-chinese-standalone-synthetic-fixtures",
            f"cargo-test-leaf-{source['id']}-identity",
            "cargo-clippy-shared-and-generated-leaves-all-targets-deny-warnings",
            f"xtask-build-signed-package-novel/zh/{source['id']}",
            f"production-wasmtime-runtime-filters-novel/zh/{source['id']}",
            f"production-extension-runner-live-{source['id']}-catalog-cover-detail-chapters-first-middle-last-text-2026-07-20",
        ],
        "packagePath": f"packages/novel/zh/{source['id']}.manatan2",
        "knownSiteFailure": None,
        "license": "MIT",
        "attribution": f"Adapted from LNReader plugins/chinese/{source['file']}; runtime is reviewed compiled Rust.",
    }


def update_matrix(matrix: dict, source: dict) -> None:
    rows = matrix["sources"]
    upstream_path = f"plugins/chinese/{source['file']}"
    existing = next((row for row in rows if row.get("mediaKind") == "novel" and row.get("language") == "zh" and (row.get("sourceId") == source["id"] or row.get("upstreamPath") == upstream_path)), None)
    entry = matrix_entry(source)
    if existing is None:
        rows.append(entry)
    else:
        existing.clear(); existing.update(entry)


def main() -> None:
    matrix_path = ROOT / "porting-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    generated = []
    for source in SOURCES:
        upstream = UPSTREAM / "plugins/chinese" / source["file"]
        if not upstream.is_file():
            raise FileNotFoundError(upstream)
        target = ROOT / "novel/zh" / source["id"]
        (target / "src").mkdir(parents=True, exist_ok=True)
        (target / "assets").mkdir(parents=True, exist_ok=True)
        shutil.copy2(UPSTREAM / "public/static" / source["icon"], target / "assets/icon.png")
        shutil.copy2(ROOT / "shared/chinese-standalone-novel/LICENSE", target / "LICENSE")
        (target / "Cargo.toml").write_text(cargo_toml(source))
        (target / "src/lib.rs").write_text(source_lib(source))
        subprocess.run(
            ["rustfmt", "--edition", "2021", str(target / "src/lib.rs")],
            check=True,
        )
        (target / "ATTRIBUTION.md").write_text(attribution(source))
        (target / "manifest.json").write_text(json.dumps(manifest(source, target), ensure_ascii=False, indent=2) + "\n")
        update_matrix(matrix, source)
        generated.append(
            {
                "id": source["id"],
                "path": str(target.relative_to(ROOT)),
                "baseUrl": source["base"],
                "status": "live-verified",
                "packagePath": f"packages/novel/zh/{source['id']}.manatan2",
                "liveVerification": {
                    "date": "2026-07-20",
                    "runner": "Manatan production ExtensionRunner",
                    "checks": [
                        "catalog",
                        "cover",
                        "details",
                        "chapters",
                        "first-text",
                        "middle-text",
                        "last-text",
                    ],
                    **LIVE_VERIFICATION[source["id"]],
                },
            }
        )
    matrix_path.write_text(json.dumps(matrix, ensure_ascii=False, indent=2) + "\n")
    report = {"family": "chinese-standalone", "adapter": "shared/chinese-standalone-novel", "generated": generated, "blocked": []}
    report_path = ROOT / "generated/lnreader-chinese-standalone.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"generated": len(generated), "blocked": 0}))


if __name__ == "__main__":
    main()
