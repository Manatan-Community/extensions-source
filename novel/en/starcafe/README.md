# StarCafe

Original implementation by Manatan Community.

Supports the public novel catalog, local title search, latest-update sorting,
details, covers, the complete chapter index, and public chapter HTML with
translator notes. The catalog is small and is paginated locally.

The public Next.js Flight data is decoded without executing website scripts.
Length-prefixed UTF-8 text records and split payload chunks are retained, so
multiline chapter text and indexes larger than the visible chapter list work.
Advance chapters remain locked; no unpublished text or access restrictions are
bypassed. Chapters are returned newest-first.

Tests: `cargo test -p manatan-novel-en-starcafe`.

The icon is the website's favicon, bundled as a PNG.
