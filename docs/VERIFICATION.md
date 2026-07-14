# Verification levels

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

