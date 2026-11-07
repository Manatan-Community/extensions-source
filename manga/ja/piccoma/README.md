# Piccoma

Japanese manga and SMARTOON, with rankings, weekday updates, search, metadata,
episodes, volume previews, login via the source WebView, and page-tile restoration.
SoraRaw is a separate, existing source; this package does not replace it.

Piccoma restricts product and reader access to networks in Japan. This package
does not bypass geographic restrictions, purchases, wait-free claims, or login.
Locked chapters retain their access state; free volume previews are identified
separately. A successful catalog request is not proof of live reader access.

## Verification

```sh
cargo test -p piccoma
cargo clippy -p piccoma --all-targets -- -D warnings
cargo run -p xtask -- build-component manga/ja/piccoma
```

After a maintainer signs the package, the following checks use Manatan's
production Wasmtime runner. Set `MANATAN_RUNTIME_ROOT` to the app checkout.
The generated harness stays under the ignored `target/` directory.

```sh
cargo run -p xtask -- runtime-test manga/ja/piccoma "$MANATAN_RUNTIME_ROOT" filters
mkdir -p target/manatan2-runtime-smoke/src/bin
cp tools/fixtures/piccoma-runtime.rs target/manatan2-runtime-smoke/src/bin/piccoma-check.rs
cargo run --manifest-path target/manatan2-runtime-smoke/Cargo.toml --bin piccoma-check -- "$PWD" fixtures
cargo run --manifest-path target/manatan2-runtime-smoke/Cargo.toml --bin piccoma-check -- "$PWD" live
```

Fixture checks exercise the actual component with deterministic HTTP responses.
Live checks use real HTTP and report geographic rejection separately. They do
not claim that paid access, reader rendering, or either mobile platform has
been verified when the reader is region-blocked.
