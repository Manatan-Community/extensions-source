// Exercise the signed guest's HTTP capability through the production runner.
// Usage: node tools/test-animejl-http-contract.mjs <Manatan checkout> [package]
// Responses are fixtures; this is a capability contract regression, not a live
// Cloudflare reproduction. Run the real app separately for platform validation.
import assert from 'node:assert/strict';
import {mkdir, copyFile, writeFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
import {spawnSync} from 'node:child_process';

assert(process.argv[2], 'Pass the Manatan checkout containing crates/manatan-extension');
const root = fileURLToPath(new URL('../', import.meta.url));
const harness = path.join(root, 'target/animejl-http-contract');
const runtime = path.resolve(process.argv[2], 'crates/manatan-extension');
const archive = path.resolve(process.argv[3] ?? path.join(root, 'dist/packages/video/es/animejl.manatan2'));
await mkdir(path.join(harness, 'src'), {recursive: true});
await writeFile(path.join(harness, 'Cargo.toml'), `[workspace]
[package]
name = "animejl-http-contract"
version = "0.0.0"
edition = "2021"
[dependencies]
manatan-extension = { path = ${JSON.stringify(runtime)}, features = ["archive", "runner"] }
serde_json = "1"
`);
await copyFile(path.join(root, 'tools/fixtures/animejl-http-contract.rs'), path.join(harness, 'src/main.rs'));
const result = spawnSync('cargo', ['run', '--quiet', '--manifest-path', path.join(harness, 'Cargo.toml'), '--', archive], {
  stdio: 'inherit',
  env: {...process.env, CARGO_TARGET_DIR: path.join(root, 'target/manatan2-runtime-smoke/target')},
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
