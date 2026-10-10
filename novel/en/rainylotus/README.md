# Rainy Lotus

Original implementation by Manatan Community for the public Lotus-theme website.

Supports most-viewed and newly added catalogs, server-side title search,
pagination, genre/tag/status/sort filters, details, covers, complete newest-first
chapter lists, and English chapter text. Original novel language is recorded
separately from reading language. Only explicit adult tags classify adult media;
Yaoi and other romance genres do not do so by themselves.

Chapter indexes follow the site's server-backed pagination, including older
pages absent from the initial HTML. Only the public chapter-list action is sent;
page-number and page-count checks prevent silently returning a partial index.

Paid chapters retain the website's access flags. Reader access is rechecked
against the server so newly released or already owned chapters can be read.
Sign-in uses the official website and a source-scoped browser profile; unlock
chapters on the website. The source never spends Lotus or Cookies automatically
and never returns a payment page as chapter text. Authenticated purchases and
reading require an authorized account and are not covered by guest live tests.

Reader HTML uses the shared WordPress sanitizer to preserve formatting and
footnotes while excluding navigation, advertisements and executable markup.
The icon is the website's published application icon.

Tests: `cargo test -p manatan-novel-en-rainylotus`.

Live guest checks: `node tools/test-rainylotus.mjs <runtime-invoker>`.
The invoker must load the signed package with the production Wasmtime runner,
accept package path, operation and JSON request as CLI arguments, and return the
JSON result on stdout. Its HTTP bridge needs GET and the read-only
`ac_novel_chapter_page` POST action. `--reading-only` reruns covers, chapter
pagination and prose comparisons independently of the catalog/filter checks.

Guest verification covered the 56-title catalog, search and filters, and three
novels with 177, 135 and 64 chapters. First, middle and newest public chapter
prose matched the official pages; paid gates and stale lock flags were checked.
