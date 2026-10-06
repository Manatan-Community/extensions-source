# OceanVeil

Original implementation by Manatan Community.

Supports the public non-mature catalog, local title search, recent additions,
details, covers, and newest-first episode lists. Regional and subscription
metadata are preserved. The public bulk catalog is used for browsing because
the site's separate search and detail endpoints require authentication.

## Authentication and playback limitations

Use source settings to sign in through OceanVeil's official website. The login
and player share a persistent, source-scoped browser profile. The official
player performs playback authorization; credentials are not entered or stored
by the extension. Only the requested episode's full, non-DRM HLS playlist is
accepted, with media-scoped cookies and an authorized-playlist check. Trailers,
premium previews, and other episodes cannot substitute for the full episode.

The site's playback authorization endpoint requires login even for episodes
marked free. Full authenticated playback has **not been live-verified** without
an authorized account. DRM-protected titles must be opened in OceanVeil's
official player; this extension does not implement DRM license playback.
Subscription, preview, regional, and DRM restrictions are never bypassed.

Tests: `cargo test -p manatan-video-en-oceanveil`.

The icon is OceanVeil's published Android touch icon.
