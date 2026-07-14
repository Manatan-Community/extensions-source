## Source

- Media kind:
- Source ID:
- Language:
- Content rating:
- Upstream source path:
- Upstream license:
- Shared framework:

## Verification

- [ ] Fixture tests cover successful and malformed responses.
- [ ] Filters, preferences, pagination, headers, cookies, and URL handling are preserved where applicable.
- [ ] The manifest declares the minimum network and host permissions.
- [ ] The `.manatan2` package contains only `manifest.json`, `extension.wasm`, and declared assets.
- [ ] `wasm-tools validate` accepts the component.
- [ ] A representative operation runs through Manatan's production Wasmtime runner.
- [ ] `porting-matrix.json` records the current status and test evidence.
