This extension ports the upstream Animetsu source from
`anime-extensions2/src/all/animetsu` by the Keiyoushi/Animiru community.

Upstream license: Apache-2.0

Original source references:
- `Animetsu.kt`
- `AnimetsuDto.kt`
- `AnimetsuFilters.kt`
- `res/mipmap-xxxhdpi/ic_launcher.png`

Manatan-specific changes:
- Replaced the Android localhost M3U8 server and PlaylistUtils flow with
  declarative `SegmentProcessing` playlist rewriting and media-offset detection.
- Added browser-session challenge bootstrap through Manatan WebView APIs so
  direct API requests can resume with synchronized cookies.
- Reworked preferences, filters, URL handling, fixtures, and tests for the
  `.manatan2` runtime.
