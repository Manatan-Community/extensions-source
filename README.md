# Manatan Community Extensions Source

Source repository for Manatan-native Rust/WASM extensions.

## Layout

- `manga/` - manga extension crates
- `video/` - video extension crates
- `novel/` - novel extension crates
- `shared/` - reusable Rust helper libraries
- `tools/` - build, validation, packaging, and index tooling

Each extension package supports exactly one media kind and builds to a `.manatan` archive.

## Build Examples

```sh
cargo run -p xtask -- validate
cargo run -p xtask -- test-examples
cargo run -p xtask -- build-all
cargo run -p xtask -- generate-index
```

Generated packages and indexes are written to `dist/`.

## Verification

`verification.json` is the gate for install indexes. `generate-index` still catalogs every built
package, but only extensions listed under `verified` are included in the app-facing
`dist/manga.min.json`, `dist/video.min.json`, and `dist/novel.min.json` feeds.

Unverified packages remain downloadable from the website catalog, `dist/catalog.min.json`, and
manual preview indexes named `dist/<media>.preview.min.json`.
Add a verification record only after the extension has been exercised in Manatan for the workflows
that matter to its media type, such as listing, thumbnails, details, chapters or episodes, reader
pages, hosters, and playback.

## Add A Source

Create a crate under one media tree:

```text
manga/<lang>/<source-id>/
video/<lang>/<source-id>/
novel/<lang>/<source-id>/
```

The crate must be a Rust `cdylib` targeting `wasm32-unknown-unknown` and must depend on:

```toml
manatan-extension = { git = "https://github.com/KolbyML/manatan-rs", default-features = false }
```

HTTP, cookies, redirects, and WebView challenge fallback are host-owned. Extensions should use
the SDK request builders, including `with_cookies_for` / `cookies_for` when a request needs shared
site cookies, instead of manually sending `Cookie` headers.

Implement the matching SDK trait:

- `MangaSource`
- `VideoSource`
- `NovelSource`

Then export with `export_manga_source!`, `export_video_source!`, or `export_novel_source!`.

## Package Rules

- One package, one media kind.
- The package extension is `.manatan`.
- `contentType` must match the root folder.
- `packageId` and source IDs must be unique across the repository.
- Build output goes to `dist/<media>/<extension-id>.manatan`.

See `tools/docs/` for authoring, packaging, indexing, and porting playbooks.
