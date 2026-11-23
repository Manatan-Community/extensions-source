# Testing and release readiness

Published extension sources are expected to work and are included directly in
their media index. The states below track implementation and test progress;
they do not control whether a published package appears in an index.

The porting matrix uses these ordered states:

- `inventoried`: upstream metadata and family are known.
- `framework-ready`: its shared family has fixture coverage.
- `implemented`: the real source behavior is implemented.
- `component-valid`: `wasm-tools validate` accepts the component and package
  digests and permissions pass validation.
- `runtime-tested`: Manatan's production Wasmtime runner loaded the component
  and executed a representative operation.
- `live-verified`: current live behavior matches the upstream source.
- `blocked-upstream`: the upstream site or implementation cannot currently be
  exercised, with a concrete reason in the matrix.

Validation checks the component world, manifest/media agreement, asset hashes,
package contents, URL permissions, and absence of undeclared files. Completion
is never inferred from compilation alone.

## Live production-path audit

`tools/test-all-sources.mjs` installs signed packages through Manatan's normal
install routes and checks source registration, every language variant's catalog,
and a representative primary-language browse/search/details/reader flow. It
fetches images through Manatan and decodes the returned bytes, so an HTML error
with status 200 does not count as a working image.

Run it against a dedicated Manatan instance and a loopback server serving the
built `dist/` directory:

```sh
node tools/test-all-sources.mjs http://127.0.0.1:CONTROL_PORT \
  http://127.0.0.1:FEED_PORT /absolute/private/path/report.json
```

An optional final argument selects comma-separated package IDs. Packages install
serially; source audits default to one worker to avoid contending for the native
browser. `AUDIT_SAMPLE_QUERIES` is a JSON object mapping package IDs to search
queries when the first popular title has no freely available chapters.

On macOS, set `MANATAN_MPV_LIBRARY` and `DYLD_FRAMEWORK_PATH` to the native
media_kit frameworks to test DASH and HLS with the app's playback engine.
`tools/test-native-playback.py` requires at least three seconds, thirty decoded
frames and a nonzero video width. The ffmpeg fallback depends on the demuxers
compiled into the local ffmpeg build.

Keep raw reports private: provider errors may contain expiring playback URLs.
Publish a sanitized source-by-source summary with exact failing stages. A
successful catalog does not establish working playback, and one unavailable
chapter does not establish a broken source. This audit does not replace mobile
UI and lifecycle testing.

## Mobile audit evidence

For an audit against the real Android or iOS native host, set
`AUDIT_DEVICE_VIDEO_UI=1`. This requests API/reader/image checks without
substituting a host video decoder for the device player. Collect native player
frames and advancing playback progress separately. Keep browser API sweeps
and native playback runs sequential on each device; a queued verification
request can contaminate startup timing. A stream list or playlist response
alone is not successful native playback. Check cold startup and saved-position
resume independently. See [the mobile retest](MOBILE-RETEST-2026-11-20.md) for
the recorded scope, restrictions, and unresolved cases.
