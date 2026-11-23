# Mobile fixes and retest — 2026-11-20

Not all sources are fully working. This report separates verified fixes, sampled success, restrictions, and unresolved failures.

Both full sweeps used the real native app containers, signed repository installation, production Wasmtime runner, local control/media/image APIs, and source browser sessions. Android used API 36/WebView 133 with the rebuilt fast-release native backend and release Flutter. iOS used the rebuilt native simulator app, release Rust backend, and debug Flutter. Each run used dedicated devices, later shut down. No catalog, HTTP, database, player, or decoder was replaced. Image bytes returned by device routes were decoded locally; remote video was never decoded on the host as a substitute for native playback.

- Android: 44 packages, 134/135 language catalogs passed.
- iOS: 44 packages, 134/135 language catalogs passed.
- Deeper checks cover representative titles in a primary language: search/latest/detail/cover/chapter pagination and reading. They do not certify every title, chapter, language, account, or region.
- Video API success means extraction/listing only. Native proof is separately labeled below.

## Verified fixes

1. WITAnime no longer mistakes captured JavaScript asset URLs for player documents. iOS bridge now preserves JSON-serializable reactive values and observes permitted child-frame/redirect navigation. HGCloud FHD video renders and advances in both native apps.
2. Deferred provider authorization runs on the control path before mpv media opening, with bounded single-flight resolution cached by source/token/signed package. Package updates and fresh provider lists invalidate old results; cancellation and failed resolutions cannot poison subsequent attempts.
3. Initial manga detail/chapter failures return real errors instead of successful uninitialized metadata/empty chapters. Existing cached data remains available offline.
4. Shanghai Fantasy searches the displayed title after removing a trailing truncation marker only when the original exact query has no match. Real search and reading pass both.
5. FuckNovelpia English reader navigates directly in its persistent browser instead of paying for two consecutive automatic browser recovery attempts. Three actual iOS chapters pass around 2 seconds each. Android verification remains unresolved.
6. ShuttleTV discovers the current public provider-list action from bounded first-party published player assets, cached against the asset list. Listing providers no longer depends on full playback authorization/codec availability. Selected-stream authorization remains intact. Native resume is still unresolved.

## Source results

| Source | Version Android / iOS | Android | iOS | Native video proof | Notes |
| --- | --- | --- | --- | --- | --- |
| 3asq | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| 52书库 | 1.0.1 / 1.0.1 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| 69书吧 | 1.0.2 / 1.0.2 | Fails: search | Reader sample passes | Not video | Android search times out twice; reader works. iOS search/reader pass. |
| AniWave (Unoriginal) | 0.2.3 / 0.2.3 | API flow passes; see playback | API flow passes; see playback | Both, earlier mobile audit | Representative flow at test time; no claim for every item. |
| AniZone | 0.1.4 / 0.1.4 | API flow passes; see playback | Fails: search, streams | Android earlier; intermittent429 | Intermittent upstream HTTP 429 at search/episode endpoints; current iOS playback extraction fails. |
| Anikoto | 0.1.13 / 0.1.13 | API flow passes; see playback | API flow passes; see playback | Both, earlier mobile audit | Representative flow at test time; no claim for every item. |
| Anime3rb | 0.1.2 / 0.1.2 | Fails: catalog | Fails: catalog | Unverified | Native browser verification times out on both. Same emulator Chrome completed verification; exact WebView difference unresolved. |
| AnimeAv1 | 0.2.5 / 0.2.5 | API flow passes; see playback | API flow passes; see playback | Both, earlier mobile audit | Representative flow at test time; no claim for every item. |
| AnimeJL | 0.1.10 / 0.1.10 | API flow passes; see playback | API flow passes; see playback | Both, earlier mobile audit | Representative flow at test time; no claim for every item. |
| AnimeOnsen | 0.1.3 / 0.1.3 | API flow passes; see playback | API flow passes; see playback | Both, earlier mobile audit | Representative flow at test time; no claim for every item. |
| AsianCTV | 0.1.4 / 0.1.4 | API flow passes; see playback | API flow passes; see playback | Both, earlier mobile audit | Representative flow at test time; no claim for every item. |
| Chikari | 1.1.3 / 1.1.3 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Cyrisia | 1.0.3 / 1.0.3 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Foxaholic | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| FuckNovelpia | 1.0.4 / 1.0.4 | Fails: reader | Reader sample passes | Not video | English reader1.0.4 passes three iOS chapters, including actual image decoding. Android reader verification still times out. |
| FuckNovelpia RAW | 1.2.3 / 1.2.3 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Hameln | 1.0.6 / 1.0.6 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Hitomi.la | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| KolNovel | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| LNORI | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| MangaDex | 1.1.3 / 1.1.3 | Reader sample passes | Reader sample passes | Not video | Yotsuba actual chapter/page reads on both. Initial sampled Solo Leveling had no available chapters; not classified as whole-source failure. |
| MangaFire | 1.3.2 / 1.3.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| MangaRaw | 1.2.0 / 1.2.0 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Mokuro Catalog | 1.1.1 / 1.1.1 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Novel Archive | 1.0.3 / 1.0.3 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| NovelHall | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| NovelPing | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| OceanVeil | 1.0.1 / 1.0.1 | Fails: streams | Fails: streams | DRM blocked | Sample explicitly requires DRM; compatible authorized playback not implemented. |
| Piccoma | 1.0.1 / 1.0.1 | Fails: details, chapters | Fails: details, chapters | Not video | Catalog/search work. Details/reader rejected outside Japan on tested connection. |
| Rainbow Mushroom's Reads | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Rainy Lotus | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Raw1001 | 1.0.3 / 1.0.3 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Rawkuma | 1.0.7 / 1.0.7 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| ReAnime | 0.1.2 / 0.1.2 | API flow passes; see playback | API flow passes; see playback | Both, earlier mobile audit | Representative flow at test time; no claim for every item. |
| Second Life Translations | 1.0.3 / 1.0.3 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Shanghai Fantasy | 1.0.4 / 1.0.4 | Reader sample passes | Reader sample passes | Not video | 1.0.4 trailing display ellipsis search fixed; exact search preserved; reader flow passes both. |
| ShuttleTV | 0.1.5 / 0.1.5 | API flow passes; see playback | API flow passes; see playback | iOS cold only; resume fails | 0.1.5 public provider-list fix:15 servers in 3.040s iOS/2.748s Android. iOS cold playback visually verified27.986→59.726s; resume74.24s stalls. Android native playback unconfirmed. Authentication/transport errors may still occur. |
| Silip | 1.0.1 / 1.0.1 | API flow passes; see playback | API flow passes; see playback | Both, earlier mobile audit | Hello, Love, Goodbye resolves streams both; earlier native movie playback verified both. Sampled Batang Quiapo season 3 episode 109 gets HTTP 502 from all 4 providers. |
| SoraRaw | 1.0.3 / 1.0.3 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| StarCafe | 1.0.2 / 1.0.2 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| Syosetu | 1.1.8 / 1.1.8 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| WIT ANIME | 0.1.4 / 0.1.4 | API flow passes; see playback | API flow passes; see playback | Both, rebuilt apps | 0.1.4 HGCloud FHD actual native playback verified both; HD resolves iOS. Two Videa alternatives remain unresolved (publisher XML error 306). |
| WTR-LAB | 1.0.3 / 1.0.3 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |
| kakuyomu | 1.0.9 / 1.0.9 | Reader sample passes | Reader sample passes | Not video | Representative flow at test time; no claim for every item. |

## Native evidence and remaining boundaries

WITAnime iOS displayed actual 1920x1080 video with progress 9.05→31.45seconds; Android rendered distinct frames with actual saved progress 8.59→16.64seconds. The supplied recordings show actual native apps. Successful media playlists alone were not counted.

ShuttleTV iOS cold playback displayed 3840x2076 video and advanced 27.986→59.726seconds without buffering/error. Closing and resuming at 74.24seconds reproducibly stalled. Readonly inspection confirmed the normal deferred-episode path and no duplicate Media.start value; that proposed cause was ruled out. Sampled proxy master/media/init/fragments returned200, and sampled MP4 timestamp bases were consistent. The exact native resume cause remains unproven; no speculative bitrate or player patch is retained. Android native playback remains unverified after a separate isolated run without a concurrent browser audit.

Anime3rb still times out in the native WebView while the same emulator’s Chrome completed verification. The Android reader challenge also loads its scripts/iframe and receives challenge responses without JavaScript exceptions, but remains verifying. This is insufficient to claim a temporary outage, a particular user-agent defect, or a universal Cloudflare fix. Shuba69 Android search is a separate unresolved browser/form timeout.

AniZone HTTP 429, Silip content-specific provider HTTP 502, OceanVeil DRM, and Piccoma Japan restriction remain visible. No retries-forever, fabricated streams, weakened network permissions, or account/region bypass is presented as a fix.

The Android WITAnime recording still shows a placeholder on the detail-page cover, although catalog covers display and the device thumbnail API returns a decoded image. That UI refresh/cache behavior is not certified fixed by the API cover check.

## Validation

- 195 Flutter player tests pass; focused Dart analysis is clean.
- Actual full server binary: 3 selected-resolution cache regressions and 1 initial manga error propagation regression pass.
- Exact production browser wrapper regression: fails before normalization, passes after.
- WITAnime 14, Shanghai Fantasy 6, FuckNovelpia shared 9, ShuttleTV 13 source tests pass.
- All 44 signed packages pass package validation. Both Git working trees pass diff whitespace checks.
- The workspace-wide rustfmt check has pre-existing differences outside this work; changed sections were formatted where applicable.

The corresponding source and package changes are included in the repository release associated with this report. Raw browser data, expiring URLs, device identities and private diagnostics are deliberately excluded.
