# Contributing

Contributions must target the `.manatan2` WebAssembly Component Model format.
Use a shared family crate when multiple sources have the same behavior; keep
leaf crates limited to configuration and genuine source-specific overrides.

Before opening a pull request:

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo run -p xtask -- validate
cargo run -p xtask -- build <media>/<lang>/<source>
```

Include parsing fixtures, the smallest required network permissions, upstream
license attribution, and an updated `porting-matrix.json` row. A compiling core
module is not sufficient: the component and package must validate, and a
representative operation must execute through Manatan's production Wasmtime
runner.
