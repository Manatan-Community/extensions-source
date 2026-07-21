# Authoring extensions

Each leaf source is a small Rust `cdylib` crate. It registers one source with
`manatan_sdk::export_extension!` and keeps the package manifest beside the
crate. Source families belong under `shared/` and expose configuration traits
plus a wrapper type that implements the appropriate Manatan source trait.

## Package files

Every package contains only declared files:

```text
manifest.json
extension.wasm
assets/icon.png       optional
```

The Wasm file is a component implementing `manatan:extensions@2.0.0`, not a
WASI program or a core module. The manifest uses schema and API version 2 and
declares one media kind. Package filenames end in `.manatan2`.

Every manifest also declares a stable publisher id and Ed25519 public key.
Source manifests use a 64-byte all-zero signature placeholder; the protected
packaging step replaces it with a signature over canonical manifest data and
the SHA-256 digest of every non-manifest archive entry. The public key must
match the configured signing key. Publisher keys are repository identities:
do not reuse another repository's private key and never add a private key to a
manifest, source tree, CI artifact, or pull-request secret.

## Source implementation

Use the SDK HTTP, cookie, browser, JavaScript, storage, asset, authentication,
and media processing services. Guests have no ambient sockets, filesystem,
process, or local-server access. Translate upstream behavior into typed Manatan
operations instead of reproducing platform APIs.

Use saved, legally permissible HTML or JSON fixtures for parser tests. Live
checks supplement fixtures; they do not replace them.

Pull-request CI uses `build-components`, which deliberately does not create a
package and never needs a publisher secret. A protected release job may use
`MANATAN_EXTENSION_SIGNING_KEY_FILE` only after the contributor code and build
tool have been reviewed. The build tool strips both signing-key variables from
the Cargo child process before any guest build script or procedural macro can
run.

## Permissions

Declare every reachable hostname, including stream and image hosts. Prefer an
exact hostname. DNS-label-aware wildcards are only appropriate when a source
really uses arbitrary subdomains. Enable browser, cookies, storage,
JavaScript, assets, or media processing only when the source uses them.
