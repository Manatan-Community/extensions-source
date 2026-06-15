# Contributing

Use the examples as the source shape for new Manatan extensions.

Before opening a pull request:

```sh
cargo fmt --check
cargo test
cargo run -p xtask -- validate
cargo run -p xtask -- test-examples
cargo run -p xtask -- build-all
cargo run -p xtask -- generate-index
cargo run -p xtask -- validate-packages
```

Rules:

- Keep one media kind per package.
- Use stable lowercase source IDs.
- Include fixture tests for parsing behavior.
- Include an icon when `manifest.json` declares one.
- Do not add generated package paths by hand. Use `xtask`.
- Keep public documentation Manatan-native.
