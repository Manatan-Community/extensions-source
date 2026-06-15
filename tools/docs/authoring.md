# Authoring Manatan Extensions

Extensions are Rust `cdylib` crates compiled to `wasm32-unknown-unknown`.

Each source lives under:

```text
manga/<lang>/<source-id>/
video/<lang>/<source-id>/
novel/<lang>/<source-id>/
```

Use the public SDK:

```toml
manatan-extension = { git = "https://github.com/KolbyML/manatan-rs", default-features = false }
```

Implement the matching trait:

- `MangaSource`
- `VideoSource`
- `NovelSource`

Export with the matching macro:

- `export_manga_source!(SOURCE)`
- `export_video_source!(SOURCE)`
- `export_novel_source!(SOURCE)`

## One Media Kind

A `.manatan` package supports exactly one media kind. Keep manga, video, and novel packages separate even when the same site offers more than one kind of content.

## Fixture Tests

Every source should keep small HTML or JSON fixtures for parser tests. Tests should cover details parsing and the primary install-time behavior for that media kind:

- manga: details, chapters, pages
- video: episodes, hosters, streams
- novel: details, chapter lists, text
