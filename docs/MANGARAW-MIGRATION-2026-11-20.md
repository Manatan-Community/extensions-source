# MangaRaw migration — 2026-11-20

Updated the former KuroNavi source to **MangaRaw 1.2.0**, using `https://mangaraw.ink`. The historical `nekoraw` package/source IDs remain unchanged, so the update keeps the same installed source association. Legacy KuroNavi/NekoRaw URLs are mapped to the new domain without fetching the unavailable old hosts. Existing title availability still depends on the publisher retaining the same slugs.

The old installed source failed through Manatan's normal catalog route because kuronavi.one did not resolve. The new signed package was installed as an update into the same dedicated database. No source, network backend, reader or Wasmtime implementation was replaced with a mock.

## Changes

- Renamed package/source display metadata and homepage/base URL; bumped the version to 1.2.0 (version code 5).
- Scoped catalog extraction to the actual listing, excluding the repeated recommendation strip. Empty listings remain empty, and pagination advances to different titles.
- Updated the current detail-status selector while retaining legacy-layout parsing.
- Scoped cover cookies to each image URL instead of requesting publisher cookies for a different CDN origin.
- Kept canonical item/chapter keys and verified old/new URL resolution through Manatan's production route.
- Fixed shared Manatan CDN discovery to parse valid unquoted HTML attributes, lazy image URLs and image metadata. Current cover origins are learned from fetched publisher responses rather than added to a site-specific allowlist. Private destinations and unrelated anchors remain excluded. Cover support therefore includes this local Manatan change as well as the source update.

## Validation

The final production-backend flow on macOS passed catalog, page 2, latest, exact-title search, details, actual cover decoding, chapter list, page list and actual reader-image decoding. First, middle and last images of chapter one also decoded through the normal reader route. Another sampled chapter (647) intermittently returned HTTP 520, then recovered to HTTP 200 both from the publisher directly and through Manatan. The server-side cause is unproven; future upstream reliability is not guaranteed. A nonexistent-title search returned no unrelated results. Legacy and new URLs resolved identically, and the registered source ID remained unchanged after updating.

All **11 source tests** and **10 shared media-policy tests** passed. Two pre-existing live transport tests in that suite are explicitly ignored by default. Repository and all **44 signed-package validations** passed, and installed package hashes matched the tested builds. This is a macOS backend/reader-route test; no mobile simulator or Flutter UI verification is claimed. It does not establish that every historical slug or chapter is still available.

The corresponding source and signed package are included in the repository release associated with this report. Operator continuity with the former domain is not asserted.
