#!/usr/bin/env python3
"""Inventory the LNReader LightNovelWorld family against its compiled adapter.

All current descriptors are broken at their declared origins, so this tool
records honest blocked matrix/report rows and deliberately emits no installable
leaf. If an origin returns, it must be freshly reviewed before removal from
`PROBE_BLOCKS` and generation of a signed package.
"""

from __future__ import annotations

import json
from pathlib import Path

from lnreader_paths import plugins_checkout


ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = plugins_checkout()
FAMILY = UPSTREAM / "plugins/multisrc/lightnovelworld"

PROBE_BLOCKS = {
    "webnovelworld": {
        "reason": (
            "The declared HTTPS origin fails certificate-chain validation "
            "(unable to get local issuer certificate), including its catalog endpoint; "
            "the HTTPS-only runtime therefore fails closed (verified 2026-07-19)."
        ),
        "test": "fresh-live-https-certificate-validation-failure-2026-07-19",
    },
    "lightnovelpubvip": {
        "reason": (
            "The root redirects from HTTPS to an unrelated HTTP parking/survey host and the "
            "catalog returns a JavaScript redirect/challenge page rather than novel cards; "
            "the compiled runtime neither downgrades transport nor executes that script "
            "(verified 2026-07-19)."
        ),
        "test": "fresh-live-http-downgrade-and-script-challenge-2026-07-19",
    },
    "lightnovelcave": {
        "reason": (
            "The root and catalog currently return HTTP 403 Cloudflare challenge pages rather "
            "than novel content; the compiled runtime fails closed and does not execute the "
            "challenge scripts (verified 2026-07-19)."
        ),
        "test": "fresh-live-http-403-script-challenge-2026-07-19",
    },
}


def matrix_entry(source: dict, probe: dict) -> dict:
    source_id = source["id"]
    return {
        "upstreamRepository": "LNReader/lnreader-plugins",
        "upstreamPath": f"plugins/multisrc/lightnovelworld/sources.json#{source_id}",
        "sourceId": source_id,
        "language": "en",
        "mediaKind": "novel",
        "framework": "lightnovelworld-lnreader",
        "requiredCapabilities": [
            "http",
            "multipart-search",
            "filters",
            "url-resolution",
            "chapter-pagination",
            "commands",
        ],
        "status": "blocked-upstream",
        "tests": [
            "cargo-test-shared-lightnovelworld-saved-fixtures",
            probe["test"],
        ],
        "packagePath": None,
        "knownSiteFailure": probe["reason"],
        "license": "MIT",
        "attribution": (
            "Source identity is from the MIT-licensed LNReader LightNovelWorld descriptor; "
            "the framework adapter is reviewed, compiled Rust."
        ),
    }


def update_matrix(matrix: dict, source: dict, probe: dict) -> None:
    source_id = source["id"]
    upstream_path = f"plugins/multisrc/lightnovelworld/sources.json#{source_id}"
    existing = next(
        (
            row
            for row in matrix["sources"]
            if row.get("mediaKind") == "novel"
            and row.get("language") == "en"
            and row.get("sourceId") == source_id
        ),
        None,
    )
    row = matrix_entry(source, probe)
    if existing is None:
        matrix["sources"].append(row)
    else:
        # The matrix schema permits one row per media/language/source ID. Keep
        # that canonical identity while replacing stale inventory evidence.
        existing.clear()
        existing.update(row)


def main() -> None:
    sources = json.loads((FAMILY / "sources.json").read_text())
    ids = {source["id"] for source in sources}
    if ids != set(PROBE_BLOCKS):
        raise RuntimeError(
            f"LightNovelWorld descriptors changed; review before generation: {sorted(ids)}"
        )

    matrix_path = ROOT / "porting-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    report = {
        "family": "lightnovelworld",
        "sharedAdapter": "shared/lightnovelworld-novel",
        "generated": [],
        "blocked": [],
    }
    for source in sources:
        probe = PROBE_BLOCKS[source["id"]]
        update_matrix(matrix, source, probe)
        report["blocked"].append(
            {
                "id": source["id"],
                "name": source["sourceName"],
                "baseUrl": source["sourceSite"].rstrip("/"),
                "reason": probe["reason"],
            }
        )

    matrix_path.write_text(json.dumps(matrix, ensure_ascii=False, indent=2) + "\n")
    report_path = ROOT / "generated/lnreader-lightnovelworld.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"generated": 0, "blocked": len(report["blocked"])}))


if __name__ == "__main__":
    main()
