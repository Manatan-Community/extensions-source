# Manatan Community extensions

Native manga, video, and novel extensions for Manatan, built as sandboxed
WebAssembly components and distributed as `.manatan2` packages.

## Repository layout

```text
manga/<lang>/<source>/       Manga extension components
video/<lang>/<source>/       Video extension components
novel/<lang>/<source>/       Novel extension components
shared/                      Reusable source-family implementations
tools/xtask/                 Build, validation, packaging, and inventory tools
docs/                        Authoring and porting documentation
porting-matrix.json          Machine-readable upstream port status
```

Extension implementations never live in the SDK. Every guest uses the public
[`manatan-sdk`](https://crates.io/crates/manatan-sdk), targets
`wasm32-unknown-unknown`, and exports `manatan:extensions@2.0.0`.

## Build an extension

```sh
rustup target add wasm32-unknown-unknown
cargo run -p xtask -- build-component manga/all/mangadex
```

`build-component` is the untrusted-contributor path: it compiles and validates
the component without access to a repository signing key. Repository
maintainers sign reviewed components separately:

```sh
cargo run -p xtask -- generate-signing-key ~/.config/manatan/extensions-source.ed25519
# Put the printed public key in each source manifest. Never commit the key file.
MANATAN_EXTENSION_SIGNING_KEY_FILE="$HOME/.config/manatan/extensions-source.ed25519" \
  cargo run -p xtask -- build manga/all/mangadex
```

The signed build command compiles and componentizes the guest, validates the
component, signs the canonical manifest and every archive entry, verifies the
finished archive, and writes a `.manatan2` package under `dist/`. The signing
variables are explicitly removed from Cargo and guest build-script
environments.

```sh
cargo test --workspace
cargo run -p xtask -- validate
cargo run -p xtask -- build-components
```

See [authoring](docs/AUTHORING.md), [porting](docs/PORTING.md), and
[verification](docs/VERIFICATION.md) before contributing.

## Community

- [Browse extensions](https://manatan-community.github.io/extensions/)
- [Join Discord](https://discord.gg/Aabn2HadF3)

Star this repository to help other Manatan users find the extension ecosystem.
