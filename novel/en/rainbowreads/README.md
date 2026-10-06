# Rainbow Mushroom's Reads

Original implementation by Manatan Community, using the shared WordPress novel
reader sanitizer.

Supports paginated popular/latest catalogs, WordPress search, details, covers,
complete chapter lists, and public chapter text. Source ordering is reversed
without collapsing split chapters that share a chapter number. Boy's Love is
not treated as an adult tag. Password-protected chapters produce an explicit
website-access message; passwords and locked content are not bypassed.

Tests: `cargo test -p manatan-novel-en-rainbowreads`.

The icon is the website's published WordPress site icon.
