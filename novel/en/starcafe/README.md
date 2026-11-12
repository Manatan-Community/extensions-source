# StarCafe

Original implementation by Manatan Community.

Supports the public novel catalog, local title search, latest-update sorting,
details, covers, the complete chapter index, and public chapter HTML with
translator notes. The catalog is small and is paginated locally.

The catalog uses the site's public Firestore index without sending site cookies
to Google. Details, the keyed chapter index, and chapter text come from public
Next.js Flight data, decoded without executing website scripts. Length-prefixed
UTF-8 text records and split payload chunks are retained.
Advance chapters remain locked; no unpublished text or access restrictions are
bypassed. Chapters are returned newest-first.

Tests: `cargo test -p manatan-novel-en-starcafe`.

The icon is the website's favicon, bundled as a PNG.
