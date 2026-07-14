# Porting sources

Port observable source behavior, not Android implementation details.

1. Identify the upstream source family and license.
2. Add shared parsing and request behavior to the family crate.
3. Prove the family with one representative source and fixtures.
4. Keep leaf sources declarative unless behavior truly differs.
5. Preserve listings, search, pagination, details, chapters or episodes,
   content, filters, preferences, authentication, headers, cookies, delays,
   URL handling, and media metadata.
6. Record status and evidence in `porting-matrix.json`.

Platform concepts map to host facilities: HTTP clients map to typed HTTP,
browser challenges map to the browser service, preferences map to typed
preferences, image transformations map to page processing, and proxy servers
map to host-owned HLS/resource processing rules.

Do not broaden the WIT ABI for source-specific behavior. Use typed operations
first and generic JSON/binary dispatch for exceptional operations. Document a
host limitation before proposing a generic SDK or runtime change.

