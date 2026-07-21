# `.manatan2` source acceptance checklist

This repository does not treat a successful compile or a non-empty catalog as
proof that a source works. A source can be published as working only after the
applicable checks below pass against the signed production package.

## Package and policy boundary

- The package is a valid WebAssembly component and imports only the reviewed
  Manatan WIT capabilities.
- The archive contains no APK, JAR, DEX, native library, downloaded program,
  raw upstream plugin, or unlisted executable asset.
- Every declared JavaScript asset is packaged, SHA-256 pinned, and callable
  only through a fixed identifier with JSON-compatible arguments and results.
- The manifest declares every HTTP, redirect, image, subtitle, and stream
  origin. The Play profile rejects HTTP and undeclared origins.
- The package id, content type, digest, publisher id, and publisher public key
  match the repository entry. A changed publisher key requires an explicit
  uninstall and trust decision.
- The content rating is explicit. Play rejects `adult` and `unknown` packages;
  safe/suggestive sources fail closed when item classification is inconclusive.
- Tampered signatures, key swaps, digest mismatches, revoked packages, legacy
  formats, and a too-new `minimumManatanVersion` are rejected.
- A third-party repository requires the Play-only unknown-sources warning and
  explicit persisted opt-in. No source repository is bundled by the app.

## Common live checks

- Popular/latest returns stable, unique items and correct pagination.
- Search returns the expected item and terminates on an out-of-range page.
- Details retain stable identity and return title, cover when available,
  creators, description, status, and non-adult tags.
- A direct item URL resolves to that item; a direct content URL resolves to its
  parent and content key where supported.
- The thumbnail request succeeds with the exact headers/cookies returned by
  the source and decodes as an image.
- An upstream redirect is rechecked against manifest permissions before it is
  followed.
- Empty, malformed, rate-limited, challenge, parked-domain, invalid-TLS, and
  removed-source responses fail with a useful error instead of fabricated
  content.

## Manga

- Chapter enumeration returns stable ordered chapter keys.
- First, middle, and last chapter page lists are non-empty and ordered.
- First, middle, and last page bytes load through the reader with required
  referer/cookies and decode as images.
- Page transforms, if any, stay within declared byte limits and produce a
  decodable image.

## Novels

- Chapter enumeration returns stable ordered chapter keys and covers chapter
  pagination when the site uses it.
- First, middle, and last chapter text loads in the reader.
- Sanitized text is non-empty, ordered, and contains no script, iframe, event
  handler, remote CSS, or navigation payload.
- Inline images load only through the host artwork proxy and declared origins.

## Video

- Episode enumeration returns stable ordered episode keys.
- At least one stream starts in the player, continues for 30 seconds, and
  seeks successfully when the upstream stream is seekable.
- HLS/DASH child playlists, segments, keys, audio, video, and subtitle URLs are
  permission-checked independently.
- The selected subtitle and alternate audio track load when advertised.
- Protected headers/cookies remain in the host proxy and are not exposed to
  the player URL.

## Platforms

- Run the signed package on a task-dedicated Android emulator using the Google
  Play distribution profile.
- Run the same package on a task-dedicated iOS Simulator.
- Confirm equivalent catalog/detail/content results and rendering on both.
- Shut down only the devices created for this test task.

## Reporting

- `live-verified` means all applicable live checks passed for the current site.
- `runtime-tested` means the signed package and operation path passed with
  deterministic fixtures but the live site could not be fully exercised.
- `blocked-upstream` records the observed site failure and does not ship a fake
  implementation. A broken novel source may remain blocked until its upstream
  is usable again.
