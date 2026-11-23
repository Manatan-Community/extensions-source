> Mobile follow-up: see [the Android and iOS retest](MOBILE-RETEST-2026-11-20.md) for current verified fixes and remaining mobile failures. The desktop results below are historical.

> Latest follow-up: the former KuroNavi source now passes as MangaRaw. See [the migration report](MANGARAW-MIGRATION-2026-11-20.md). Earlier results below are historical.

> Follow-up: KolNovel, ShuttleTV, Silip and WITAnime now pass representative flows. See [the upstream comparison](UPSTREAM-COMPARISON-2026-11-20.md). The initial results below are retained as historical evidence.

# Manatan SDK and live-source audit — 2026-11-20

All **44 signed packages / 135 registered language variants** were exercised through a dedicated Manatan instance. **35 packages passed representative reading/playback flows**; the remaining rows below state the verified restriction or unresolved failing boundary.

## Scope and reproducibility

This used Manatan’s normal repository install routes, package signature verification, production Wasmtime guest runner, local HTTP/media routes, source-scoped cookie/network policies, native desktop CEF and the app’s media_kit libmpv. No fake catalog, HTTP backend, source implementation or decoder was substituted. Each language variant’s catalog was requested. Deeper browse/page 2/latest/search/details/cover/chapter checks used a representative primary language, not every chapter and title in all languages. Manga image bytes were decoded; novel chapters were fetched/materialized; native video success required at least 3 seconds, 30 frames and nonzero width.

Tests ran on **macOS**, not an Android emulator, iOS simulator or the Flutter UI. API/native media success is not mobile UI certification, nor a check of every audio language or player control. Raw diagnostic data and expiring provider URLs remain private; this report intentionally contains no clearance cookies or signed media URLs.

## SDK adoption

- Pinned the workspace to SDK `d13370a1abc220eda0a805ca9eef0a22d7cadf10`; rebuilt all packages and updated their versions and local indexes.
- Added combined refresh operations to 13 source packages, preserving flags and omitted host state.
- Added MangaDex page-request resolution with a legacy fallback across 61 language variants.
- Added ShuttleTV related-title resolution; a real request returned 20 recommendations.
- Used scoped browser sessions, checked CDN discovery/redirects, request capture and source-provided image requests where needed. Origins remain permission checked; local/private destinations are rejected.

## Proven fixes

Source fixes include 69Shuba’s actual GBK search form, Hameln’s changed episode-list markup, Raw1001 loading-link filtering, Rainbow Reads pagination, VOE’s current identity-preserving alias, WTR-LAB content responses/display labels, first-party WordPress cover selection, and WITAnime’s current publisher navigation and deferred server selection. These are verified only to the extent stated in each row.

Shared Manatan fixes were necessary because real failures occurred below source code: CDN TLS/browser identity defaults, request-backed cover bytes, invalidated derived page metadata after package updates, native request/header ownership during navigation, network interception servicing during asynchronous browser waits, a total browser deadline including queue time, readiness recovery after redirects destroy script contexts, and correct final document URLs without requiring cookie sharing. Desktop CEF 149’s renderer redirect header regression ([upstream issue](https://github.com/chromiumembedded/cef/issues/4189), [upstream repair](https://github.com/chromiumembedded/cef/pull/4190)) was reproduced locally and against a live verification script; the narrow header-ownership repair retains CORS enforcement and the project’s patched Linux CEF 149 runtime.

## Source-by-source results

| Source | Type | Version | Result | Evidence / remaining limitation |
| --- | --- | --- | --- | --- |
| 3asq | manga | 1.0.2 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| 52书库 | novel | 1.0.1 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| 69书吧 | novel | 1.0.2 | Verified sample | Chinese search now submits the publisher’s actual GBK form in the native browser; chapter reading passed. |
| AniWave (Unoriginal) | video | 0.2.3 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| AniZone | video | 0.1.4 | Intermittent | Earlier native playback decoded successfully, but later search/player requests returned HTTP 429. Current reliability remains rate-limit dependent; do not classify this as wholly working or globally down. |
| Anikoto | video | 0.1.12 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| Anime3rb | video | 0.1.2 | Unresolved | Catalog browser verification did not finish within the bounded request. No verified upstream outage or completed clearance; catalog and playback remain unverified. |
| AnimeAv1 | video | 0.2.4 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| AnimeJL | video | 0.1.9 | Verified sample | Current title Hotaru no Yomeiri played in the native engine. Older sampled popular-title mirrors were parked/unavailable; this does not establish that every title works. |
| AnimeOnsen | video | 0.1.3 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| AsianCTV | video | 0.1.4 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| Chikari | novel | 1.1.3 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| Cyrisia | novel | 1.0.3 | Verified sample | Current exact-title search and chapter reading passed; source-provided placeholder covers do not count as downloaded artwork. |
| Foxaholic | novel | 1.0.1 | Verified sample | Exact-title search and chapter reading passed. A generic two-word query is not a reliable first-page search assertion. |
| FuckNovelpia | novel | 1.0.3 | Verified sample | Browser-backed chapter reading passed; consume verified HTML in its browser context rather than relying on HTTP clearance replay. |
| FuckNovelpia RAW | novel | 1.2.3 | Verified sample | Actual archive materialization and reading passed through the normal library route. |
| Hameln | novel | 1.0.6 | Verified sample | Updated episode-list markup was missing from the parser; current chapter list and reading now passed. |
| Hitomi.la | manga | 1.0.2 | Verified sample | All 26 language catalogs passed; one primary-language page/reader sample decoded. |
| KolNovel | novel | 1.0.2 | Blocked on tested connection | The actual catalog endpoint returns HTTP 503; the direct reference response reports maintenance/capacity. No confirmed working replacement; this is not proof of permanent shutdown. |
| KuroNavi | manga | 1.1.1 | Blocked: DNS | kuronavi.one is NXDOMAIN from both Google and Cloudflare DNS. No verified replacement domain; package modernization cannot restore an absent publisher endpoint. |
| LNORI | novel | 1.0.2 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| MangaDex | manga | 1.1.3 | Verified sample | All 61 language catalogs passed; an available English chapter and image decoded. Chapter availability varies by title/language. |
| MangaFire | manga | 1.3.2 | Verified sample | All 7 language catalogs passed; primary-language chapter image decoded. |
| Mokuro Catalog | manga | 1.1.1 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| Novel Archive | novel | 1.0.3 | Verified sample | Catalog, search, details, cover decode, chapter pagination and reading passed. Missing shared CDN User-Agent caused the cover’s 403; default and explicit browser identities are regression tested. |
| NovelHall | novel | 1.0.1 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| NovelPing | novel | 1.0.1 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| OceanVeil | video | 1.0.1 | Restricted sample | The sampled title explicitly requires DRM. Catalog/details work; this implementation cannot supply native DRM playback and directs the user to the official player. |
| Piccoma | manga | 1.0.1 | Restricted on tested connection | Public catalog/search/cover work, but reader/details access is Japan restricted from this connection. No Japan egress/account was available to verify permitted reader access; no bypass is claimed. |
| Rainbow Mushroom's Reads | novel | 1.0.1 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| Rainy Lotus | novel | 1.0.1 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| Raw1001 | manga | 1.0.3 | Verified sample | Old loading placeholders produced non-image pages. Verified old signed package failure and fixed package upgrade without resetting the database; downloaded manifests and reading progress are retained. |
| Rawkuma | manga | 1.0.6 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| ReAnime | video | 0.1.2 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| Second Life Translations | novel | 1.0.2 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| Shanghai Fantasy | novel | 1.0.2 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| ShuttleTV | video | 0.1.4 | Unresolved playback | Catalog/search/details/cover/episodes and 20 related titles work. The real browser captures manifests, but normal HTTP manifest probes fail at the network layer. Exact provider transport cause is not yet established; not labeled an outage. |
| Silip | video | 1.0.1 | Unresolved playback | Catalog/search/details/cover/episodes work. The VidCore embed did not expose a playable stream through the production browser path. No working extractor or native decode verified. |
| SoraRaw | manga | 1.0.3 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| StarCafe | novel | 1.0.2 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| Syosetu | novel | 1.1.8 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |
| WIT ANIME | video | 0.1.3 | Unresolved playback | Migrated to current publisher site; catalog/page 2/latest/search/details/cover/episodes/server selection work. Actual native playback still fails during deferred hoster extraction. Current-site, packed-config, redirected-header and shared browser-wait repairs did not establish native playback. Deferred hoster extraction/transport still fails; the exact remaining provider interaction is not established, and no outage is claimed. |
| WTR-LAB | novel | 1.0.3 | Verified sample | Updated content_url chapter responses and encoded display text; actual chapter content read through Manatan. |
| kakuyomu | novel | 1.0.9 | Verified sample | Representative browse/search/details and actual chapter reading or native playback passed. |

## Validation and delivery

The source workspace’s **338 tests passed**. All **44 signed packages** passed manifest/component/package validation. **76 focused Manatan checks passed**: 54 browser tests, 9 media-policy tests, 1 cache test, 10 CEF tests, and 2 explicit live TLS/User-Agent tests. The audit tools are `tools/test-all-sources.mjs` and `tools/test-native-playback.py`; see `docs/VERIFICATION.md` for use and limits. An actual old-package → new-package Raw1001 upgrade was also tested without wiping user state.

The root workspace-wide rustfmt check reports pre-existing differences outside this change; changed code was formatted where applicable, the source workspace formatting check passes, and `git diff --check` passes.

The corresponding source and package changes are included in the repository release associated with this report. A passing sample means the recorded flow worked at test time, not that every future title, mirror, region, account or platform is guaranteed to work. Unresolved rows must not be advertised as fixed.
