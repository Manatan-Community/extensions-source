# Packages And Indexes

`cargo run -p xtask -- build-all` builds every extension and writes packages to:

```text
dist/<media>/<extension-id>.manatan
```

Each package contains:

- `manifest.json`
- `module.wasm`
- `filters.json` when present
- `preferences.json` when present
- declared assets such as `assets/icon.svg`

`cargo run -p xtask -- generate-index` writes deterministic minified indexes:

- `dist/manga.min.json`
- `dist/video.min.json`
- `dist/novel.min.json`
- `dist/manga.preview.min.json`
- `dist/video.preview.min.json`
- `dist/novel.preview.min.json`
- `dist/catalog.min.json`

The media indexes are install feeds and only include extensions listed in `verification.json`.
The preview media indexes and `dist/catalog.min.json` include every built package with a
`verified` flag and optional verification metadata so the website can show unverified packages
without putting them in the app-facing indexes.

Each index entry includes ID, name, media kind, language, content rating, version, package path,
package URL, SHA-256, size, icon path, icon URL, source IDs, and verification status.

## Validation

Run:

```sh
cargo run -p xtask -- validate
cargo run -p xtask -- validate-packages
```

Validation fails on duplicate extension IDs, duplicate source IDs, missing icons, invalid media kind, invalid package extension, missing archive files, and multi-media package declarations.
