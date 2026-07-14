use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsStr,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{ensure, Context, Result};
use manatan_sdk::manifest::{ContentType, Manifest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, CompressionMethod, DateTime, ZipArchive, ZipWriter};

const SDK_GIT_URL: &str = "https://github.com/KolbyML/Manatan-SDK";
const MEDIA: [&str; 3] = ["manga", "video", "novel"];

#[derive(Clone, Debug)]
struct ExtensionDir {
    media: String,
    lang: String,
    id: String,
    path: PathBuf,
    manifest: Manifest,
    crate_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortingMatrix {
    schema_version: u32,
    generated_at: String,
    sources: Vec<PortingRow>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortingRow {
    upstream_repository: String,
    upstream_path: String,
    source_id: String,
    language: String,
    media_kind: String,
    framework: String,
    required_capabilities: Vec<String>,
    status: String,
    tests: Vec<String>,
    package_path: Option<String>,
    #[serde(default)]
    known_site_failure: Option<String>,
    license: String,
    #[serde(default)]
    attribution: String,
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("validate") => validate_repository(),
        Some("build") => build_one(&required_arg(args.next(), "extension path")?),
        Some("build-all") => build_all(),
        Some("generate-index") => generate_indexes(),
        Some("publish") => publish(&required_arg(args.next(), "generated repository path")?),
        Some("validate-packages") => validate_packages(),
        Some("runtime-test") => runtime_test(
            &required_arg(args.next(), "extension path")?,
            args.next().as_deref(),
            args.next().as_deref(),
            args.next().as_deref(),
        ),
        Some("inventory-upstreams") => inventory_upstreams(args.collect()),
        Some("matrix-update") => matrix_update(args.collect()),
        Some("matrix") => validate_matrix(),
        _ => {
            eprintln!(
                "usage: cargo run -p xtask -- <validate|build PATH|build-all|generate-index|publish GENERATED_REPO|validate-packages|runtime-test PATH [RUNTIME_ROOT] [OPERATION] [REQUEST_JSON]|inventory-upstreams VIDEO_ROOT MANGA_ROOT NOVEL_ROOT|matrix-update PATH STATUS [TEST...] [--failure REASON]|matrix>"
            );
            Ok(())
        }
    }
}

fn required_arg(value: Option<String>, name: &str) -> Result<String> {
    value.with_context(|| format!("missing {name}"))
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("xtask is nested under the workspace")
        .to_path_buf()
}

fn discover_extensions() -> Result<Vec<ExtensionDir>> {
    let root = root();
    let mut extensions = Vec::new();
    for media in MEDIA {
        let media_dir = root.join(media);
        if !media_dir.exists() {
            continue;
        }
        for lang in child_directories(&media_dir)? {
            for path in child_directories(&lang)? {
                let manifest_path = path.join("manifest.json");
                let cargo_path = path.join("Cargo.toml");
                if !manifest_path.is_file() && !cargo_path.is_file() {
                    continue;
                }
                ensure!(
                    manifest_path.is_file(),
                    "{} is missing manifest.json",
                    path.display()
                );
                ensure!(
                    cargo_path.is_file(),
                    "{} is missing Cargo.toml",
                    path.display()
                );
                let manifest: Manifest = serde_json::from_slice(&fs::read(&manifest_path)?)
                    .with_context(|| format!("parse {}", manifest_path.display()))?;
                let cargo: toml::Value = fs::read_to_string(&cargo_path)?
                    .parse()
                    .with_context(|| format!("parse {}", cargo_path.display()))?;
                let crate_name = cargo
                    .get("package")
                    .and_then(|value| value.get("name"))
                    .and_then(toml::Value::as_str)
                    .with_context(|| format!("{} has no package.name", cargo_path.display()))?
                    .to_owned();
                extensions.push(ExtensionDir {
                    media: media.to_owned(),
                    lang: lang.file_name().unwrap().to_string_lossy().into_owned(),
                    id: path.file_name().unwrap().to_string_lossy().into_owned(),
                    path,
                    manifest,
                    crate_name,
                });
            }
        }
    }
    extensions.sort_by(|left, right| {
        (&left.media, &left.lang, &left.id).cmp(&(&right.media, &right.lang, &right.id))
    });
    Ok(extensions)
}

fn child_directories(path: &Path) -> Result<Vec<PathBuf>> {
    let mut directories = fs::read_dir(path)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            !path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with('.')
        })
        .collect::<Vec<_>>();
    directories.sort();
    Ok(directories)
}

fn validate_repository() -> Result<()> {
    let extensions = discover_extensions()?;
    ensure!(
        !extensions.is_empty(),
        "no .manatan2 extension crates were found"
    );
    let mut package_ids = BTreeSet::new();
    let mut source_ids = BTreeSet::new();
    for extension in &extensions {
        validate_extension(extension)?;
        ensure!(
            package_ids.insert(extension.manifest.id.clone()),
            "duplicate package id {}",
            extension.manifest.id
        );
        for source in &extension.manifest.sources {
            ensure!(
                source_ids.insert(source.id.clone()),
                "duplicate source id {}",
                source.id
            );
        }
    }
    validate_matrix_against(&extensions)?;
    reject_retired_format_references()?;
    println!("validated {} .manatan2 extension crates", extensions.len());
    Ok(())
}

fn validate_extension(extension: &ExtensionDir) -> Result<()> {
    extension
        .manifest
        .validate()
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("validate {}", extension.path.display()))?;
    ensure!(
        extension.manifest.wasm == "extension.wasm",
        "{} must use extension.wasm",
        extension.id
    );
    ensure!(
        extension.manifest.id == extension.id,
        "package id must match directory {}",
        extension.id
    );
    ensure!(
        content_type_name(extension.manifest.content_type) == extension.media,
        "{} contentType does not match {}",
        extension.id,
        extension.media
    );
    ensure!(
        extension
            .manifest
            .sources
            .iter()
            .all(|source| source.lang == extension.lang),
        "{} source language does not match {}",
        extension.id,
        extension.lang
    );
    ensure!(
        extension
            .manifest
            .license
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        "{} must declare its license",
        extension.id
    );
    ensure!(
        extension
            .manifest
            .repository
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        "{} must declare its repository",
        extension.id
    );
    let cargo: toml::Value = fs::read_to_string(extension.path.join("Cargo.toml"))?.parse()?;
    let crate_types = cargo
        .get("lib")
        .and_then(|value| value.get("crate-type"))
        .and_then(toml::Value::as_array)
        .context("extension Cargo.toml must declare [lib].crate-type")?;
    ensure!(
        crate_types
            .iter()
            .any(|value| value.as_str() == Some("cdylib")),
        "{} must build a cdylib",
        extension.id
    );
    let declared_assets = extension
        .manifest
        .assets
        .iter()
        .map(|asset| asset.path.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(icon) = extension.manifest.icon.as_deref() {
        ensure!(
            declared_assets.contains(icon),
            "{} icon must be a declared asset",
            extension.id
        );
    }
    for asset in &extension.manifest.assets {
        let path = extension.path.join(&asset.path);
        ensure!(path.is_file(), "{} is missing", path.display());
        let expected = asset
            .sha256
            .as_deref()
            .context("every asset must declare sha256")?;
        ensure!(
            sha256(&fs::read(&path)?) == expected.to_ascii_lowercase(),
            "{} digest mismatch",
            path.display()
        );
    }
    for pattern in &extension.manifest.permissions.network.allow {
        validate_network_pattern(pattern).with_context(|| {
            format!("invalid network permission {pattern:?} in {}", extension.id)
        })?;
    }
    Ok(())
}

fn validate_network_pattern(pattern: &str) -> Result<()> {
    let (scheme, authority) = pattern
        .split_once("://")
        .context("network entries must be URL origins such as https://example.com")?;
    ensure!(
        matches!(scheme, "http" | "https"),
        "network entry scheme must be http or https"
    );
    ensure!(
        !authority.contains(['/', '?', '#', '@']),
        "network entries must be origins without paths, queries, fragments, or credentials"
    );
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _port)| host);
    ensure!(
        host != "*" && host != "*.*",
        "broad wildcards are forbidden"
    );
    let host = host.strip_prefix("*.").unwrap_or(host);
    ensure!(
        host.contains('.'),
        "hostname must contain a registrable suffix"
    );
    ensure!(
        host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        }),
        "hostname labels are invalid"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_network_pattern;

    #[test]
    fn validates_network_url_origins() {
        assert!(validate_network_pattern("https://example.com").is_ok());
        assert!(validate_network_pattern("https://*.example.com").is_ok());
        assert!(validate_network_pattern("http://example.com:8080").is_ok());
    }

    #[test]
    fn rejects_non_origin_network_permissions() {
        assert!(validate_network_pattern("example.com").is_err());
        assert!(validate_network_pattern("ftp://example.com").is_err());
        assert!(validate_network_pattern("https://example.com/path").is_err());
        assert!(validate_network_pattern("https://*").is_err());
    }
}

fn validate_matrix() -> Result<()> {
    validate_matrix_against(&discover_extensions()?)
}

fn validate_matrix_against(extensions: &[ExtensionDir]) -> Result<()> {
    let path = root().join("porting-matrix.json");
    let matrix: PortingMatrix = serde_json::from_slice(&fs::read(&path)?)?;
    ensure!(
        matrix.schema_version == 1,
        "unsupported porting matrix schema"
    );
    let allowed = [
        "inventoried",
        "framework-ready",
        "implemented",
        "component-valid",
        "runtime-tested",
        "live-verified",
        "blocked-upstream",
    ];
    let mut keys = BTreeSet::new();
    for row in &matrix.sources {
        ensure!(
            allowed.contains(&row.status.as_str()),
            "invalid status {}",
            row.status
        );
        ensure!(
            !row.upstream_path.is_empty(),
            "{} has no upstream path",
            row.source_id
        );
        ensure!(!row.license.is_empty(), "{} has no license", row.source_id);
        ensure!(
            keys.insert((&row.media_kind, &row.language, &row.source_id)),
            "duplicate matrix row {}/{}/{}",
            row.media_kind,
            row.language,
            row.source_id
        );
    }
    for extension in extensions {
        for source in &extension.manifest.sources {
            let row = matrix.sources.iter().find(|row| {
                row.media_kind == extension.media
                    && row.language == extension.lang
                    && row.source_id == source.id
            });
            ensure!(row.is_some(), "{} has no porting matrix row", source.id);
        }
    }
    println!("validated {} porting matrix rows", matrix.sources.len());
    Ok(())
}

fn inventory_upstreams(arguments: Vec<String>) -> Result<()> {
    ensure!(
        arguments.len() == 3,
        "inventory-upstreams requires the anime, manga, and novel upstream roots"
    );
    let matrix_path = root().join("porting-matrix.json");
    let current: PortingMatrix = if matrix_path.is_file() {
        serde_json::from_slice(&fs::read(&matrix_path)?)?
    } else {
        PortingMatrix {
            schema_version: 1,
            generated_at: String::new(),
            sources: Vec::new(),
        }
    };
    let generated_at =
        env::var("MANATAN_INVENTORY_TIMESTAMP").unwrap_or_else(|_| current.generated_at.clone());
    let mut rows = current
        .sources
        .into_iter()
        .map(|row| {
            (
                (
                    row.media_kind.clone(),
                    row.language.clone(),
                    row.source_id.clone(),
                ),
                row,
            )
        })
        .collect::<BTreeMap<_, _>>();
    inventory_android_tree(
        Path::new(&arguments[0]),
        "yuzono/anime-extensions",
        "video",
        "Apache-2.0",
        &mut rows,
    )?;
    inventory_android_tree(
        Path::new(&arguments[1]),
        "keiyoushi/extensions-source",
        "manga",
        "Apache-2.0",
        &mut rows,
    )?;
    inventory_ireader_tree(Path::new(&arguments[2]), &mut rows)?;
    let matrix = PortingMatrix {
        schema_version: 1,
        generated_at,
        sources: rows.into_values().collect(),
    };
    fs::write(&matrix_path, serde_json::to_vec_pretty(&matrix)?)?;
    println!("inventoried {} upstream sources", matrix.sources.len());
    Ok(())
}

fn matrix_update(arguments: Vec<String>) -> Result<()> {
    ensure!(
        arguments.len() >= 2,
        "matrix-update requires media/lang/source, status, optional test evidence, and optional --failure REASON"
    );
    let parts = arguments[0].split('/').collect::<Vec<_>>();
    ensure!(
        parts.len() == 3 && MEDIA.contains(&parts[0]),
        "matrix path must be media/lang/source"
    );
    let allowed = [
        "inventoried",
        "framework-ready",
        "implemented",
        "component-valid",
        "runtime-tested",
        "live-verified",
        "blocked-upstream",
    ];
    ensure!(
        allowed.contains(&arguments[1].as_str()),
        "invalid matrix status {}",
        arguments[1]
    );
    let path = root().join("porting-matrix.json");
    let mut matrix: PortingMatrix = serde_json::from_slice(&fs::read(&path)?)?;
    let row = matrix
        .sources
        .iter_mut()
        .find(|row| {
            row.media_kind == parts[0] && row.language == parts[1] && row.source_id == parts[2]
        })
        .with_context(|| format!("{} has no matrix row", arguments[0]))?;
    let failure_position = arguments.iter().position(|value| value == "--failure");
    let failure = failure_position
        .map(|position| {
            arguments
                .get(position + 1)
                .cloned()
                .context("--failure requires a reason")
        })
        .transpose()?;
    if let Some(position) = failure_position {
        ensure!(
            position + 2 == arguments.len(),
            "--failure REASON must be the final matrix-update arguments"
        );
    }
    row.status = arguments[1].clone();
    row.tests = arguments[2..failure_position.unwrap_or(arguments.len())].to_vec();
    row.known_site_failure = failure;
    row.package_path = matches!(
        row.status.as_str(),
        "component-valid" | "runtime-tested" | "live-verified"
    )
    .then(|| {
        format!(
            "packages/{}/{}/{}.manatan2",
            row.media_kind, row.language, row.source_id
        )
    });
    fs::write(path, serde_json::to_vec_pretty(&matrix)?)?;
    println!("updated {} to {}", arguments[0], arguments[1]);
    Ok(())
}

fn inventory_android_tree(
    upstream: &Path,
    repository: &str,
    media: &str,
    license: &str,
    rows: &mut BTreeMap<(String, String, String), PortingRow>,
) -> Result<()> {
    let source_root = upstream.join("src");
    ensure!(
        source_root.is_dir(),
        "{} has no src directory",
        upstream.display()
    );
    for language_dir in child_directories(&source_root)? {
        let language = language_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for source_dir in child_directories(&language_dir)? {
            if !contains_kotlin_or_build_file(&source_dir) {
                continue;
            }
            let source_id = source_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let framework =
                infer_android_framework(&source_dir).unwrap_or_else(|| "standalone".into());
            let key = (media.to_owned(), language.clone(), source_id.clone());
            rows.entry(key).or_insert_with(|| PortingRow {
                upstream_repository: repository.to_owned(),
                upstream_path: format!("src/{language}/{source_id}"),
                source_id: source_id.clone(),
                language: language.clone(),
                media_kind: media.to_owned(),
                framework,
                required_capabilities: default_capabilities(media),
                status: "inventoried".into(),
                tests: Vec::new(),
                package_path: None,
                known_site_failure: None,
                license: license.to_owned(),
                attribution: format!("Ported from {repository}."),
            });
        }
    }
    Ok(())
}

fn inventory_ireader_tree(
    upstream: &Path,
    rows: &mut BTreeMap<(String, String, String), PortingRow>,
) -> Result<()> {
    let source_root = upstream.join("sources");
    ensure!(
        source_root.is_dir(),
        "{} has no sources directory",
        upstream.display()
    );
    for language_dir in child_directories(&source_root)? {
        let language = language_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if matches!(language.as_str(), "common" | "multisrc") {
            continue;
        }
        for source_dir in child_directories(&language_dir)? {
            if contains_kotlin_file(&source_dir) {
                insert_ireader_row(rows, &language, &source_dir, "standalone", None);
            }
        }
    }
    let multisrc = source_root.join("multisrc");
    if multisrc.is_dir() {
        for family_dir in child_directories(&multisrc)? {
            let family = family_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            for source_dir in child_directories(&family_dir)? {
                if !contains_kotlin_file(&source_dir) {
                    continue;
                }
                let language = infer_ireader_language(&source_dir).unwrap_or_else(|| "all".into());
                insert_ireader_row(rows, &language, &source_dir, &family, Some(&family));
            }
        }
    }
    Ok(())
}

fn insert_ireader_row(
    rows: &mut BTreeMap<(String, String, String), PortingRow>,
    language: &str,
    source_dir: &Path,
    framework: &str,
    multisrc_family: Option<&str>,
) {
    let source_id = source_dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let key = ("novel".to_owned(), language.to_owned(), source_id.clone());
    rows.entry(key).or_insert_with(|| PortingRow {
        upstream_repository: "IReaderorg/IReader-extensions".into(),
        upstream_path: multisrc_family.map_or_else(
            || format!("sources/{language}/{source_id}"),
            |family| format!("sources/multisrc/{family}/{source_id}"),
        ),
        source_id: source_id.clone(),
        language: language.to_owned(),
        media_kind: "novel".into(),
        framework: framework.to_owned(),
        required_capabilities: default_capabilities("novel"),
        status: "inventoried".into(),
        tests: Vec::new(),
        package_path: None,
        known_site_failure: None,
        license: "MPL-2.0".into(),
        attribution: "Ported from IReaderorg/IReader-extensions.".into(),
    });
}

fn contains_kotlin_or_build_file(path: &Path) -> bool {
    contains_kotlin_file(path)
        || path.join("build.gradle").is_file()
        || path.join("build.gradle.kts").is_file()
}

fn contains_kotlin_file(path: &Path) -> bool {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry.file_type().is_file()
                && entry.path().extension().and_then(OsStr::to_str) == Some("kt")
        })
}

fn infer_android_framework(path: &Path) -> Option<String> {
    for entry in WalkDir::new(path)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(OsStr::to_str) != Some("kt")
        {
            continue;
        }
        let contents = fs::read_to_string(entry.path()).ok()?;
        for line in contents.lines() {
            let marker = ".multisrc.";
            let Some(index) = line.find(marker) else {
                continue;
            };
            let suffix = &line[index + marker.len()..];
            let family = suffix.split('.').next().unwrap_or_default();
            if !family.is_empty() {
                return Some(family.to_owned());
            }
        }
    }
    None
}

fn infer_ireader_language(path: &Path) -> Option<String> {
    for entry in WalkDir::new(path)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(OsStr::to_str) != Some("kt")
        {
            continue;
        }
        let contents = fs::read_to_string(entry.path()).ok()?;
        for marker in ["lang = \"", "lang=\""] {
            if let Some(start) = contents.find(marker) {
                let value = &contents[start + marker.len()..];
                if let Some(end) = value.find('"') {
                    return Some(value[..end].to_owned());
                }
            }
        }
    }
    None
}

fn default_capabilities(media: &str) -> Vec<String> {
    let mut values = vec!["http".into(), "filters".into(), "url-resolution".into()];
    match media {
        "video" => values.extend(["hoster-resolution".into(), "media-processing".into()]),
        "manga" => values.push("page-processing".into()),
        "novel" => values.extend(["chapter-pagination".into(), "commands".into()]),
        _ => {}
    }
    values
}

fn reject_retired_format_references() -> Result<()> {
    let retired_suffix = [".", "manatan"].concat();
    for entry in WalkDir::new(root()).into_iter().filter_entry(|entry| {
        entry.file_name() != OsStr::new(".git") && entry.file_name() != OsStr::new("target")
    }) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let extension = entry
            .path()
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or_default();
        if !matches!(extension, "md" | "json" | "toml" | "rs" | "yml" | "yaml") {
            continue;
        }
        let contents = fs::read_to_string(entry.path())?;
        ensure!(
            !contents.contains(&format!("{retired_suffix}\""))
                && !contents.contains(&format!("{retired_suffix}`"))
                && !contents.contains(&format!("{retired_suffix} ")),
            "{} references the retired package format",
            entry.path().display()
        );
    }
    Ok(())
}

fn build_one(relative: &str) -> Result<()> {
    let relative = relative.trim_matches('/');
    let extension = discover_extensions()?
        .into_iter()
        .find(|extension| {
            format!("{}/{}/{}", extension.media, extension.lang, extension.id) == relative
        })
        .with_context(|| format!("unknown extension {relative}"))?;
    validate_extension(&extension)?;
    let package = build_extension(&extension)?;
    println!("built {}", package.display());
    Ok(())
}

fn build_all() -> Result<()> {
    let extensions = discover_extensions()?;
    validate_repository()?;
    for extension in &extensions {
        let package = build_extension(extension)?;
        println!("built {}", package.display());
    }
    generate_indexes()
}

fn build_extension(extension: &ExtensionDir) -> Result<PathBuf> {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(root());
    if let Some(path) = env::var_os("MANATAN_SDK_PATH") {
        let path = PathBuf::from(path).canonicalize()?;
        command.arg("--config").arg(format!(
            "patch.\"{SDK_GIT_URL}\".manatan-sdk.path='{}'",
            path.display()
        ));
    }
    command.args([
        "build",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
        "-p",
        &extension.crate_name,
    ]);
    run(&mut command, "compile core WebAssembly module")?;

    let core_name = extension.crate_name.replace('-', "_");
    let core = root()
        .join("target/wasm32-unknown-unknown/release")
        .join(format!("{core_name}.wasm"));
    ensure!(core.is_file(), "cargo did not produce {}", core.display());
    let component_dir = root().join("target/manatan2-components");
    fs::create_dir_all(&component_dir)?;
    let component = component_dir.join(format!("{}.wasm", extension.manifest.id));
    run(
        Command::new("wasm-tools")
            .args(["component", "new"])
            .arg(&core)
            .arg("-o")
            .arg(&component),
        "componentize extension",
    )?;
    run(
        Command::new("wasm-tools")
            .args(["validate", "--features", "component-model"])
            .arg(&component),
        "validate WebAssembly component",
    )?;
    let wit = Command::new("wasm-tools")
        .args(["component", "wit"])
        .arg(&component)
        .output()
        .context("inspect component WIT")?;
    ensure!(wit.status.success(), "wasm-tools component wit failed");
    let wit = String::from_utf8(wit.stdout)?;
    ensure!(
        wit.contains("manatan:extensions") && wit.contains("@2.0.0"),
        "{} does not target manatan:extensions@2.0.0",
        extension.id
    );

    let output = package_path(extension);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    write_package(extension, &component, &output)?;
    validate_package(extension, &output)?;
    Ok(output)
}

fn run(command: &mut Command, action: &str) -> Result<()> {
    let status = command
        .stdin(Stdio::null())
        .status()
        .with_context(|| action.to_owned())?;
    ensure!(status.success(), "{action} failed with {status}");
    Ok(())
}

fn write_package(extension: &ExtensionDir, component: &Path, output: &Path) -> Result<()> {
    let file = File::create(output)?;
    let mut zip = ZipWriter::new(file);
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default());
    let compressed = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default());
    zip.start_file("manifest.json", compressed)?;
    zip.write_all(&fs::read(extension.path.join("manifest.json"))?)?;
    zip.start_file(&extension.manifest.wasm, stored)?;
    zip.write_all(&fs::read(component)?)?;
    let mut assets = extension.manifest.assets.iter().collect::<Vec<_>>();
    assets.sort_by(|left, right| left.path.cmp(&right.path));
    for asset in assets {
        zip.start_file(&asset.path, compressed)?;
        zip.write_all(&fs::read(extension.path.join(&asset.path))?)?;
    }
    zip.finish()?;
    Ok(())
}

fn validate_packages() -> Result<()> {
    let extensions = discover_extensions()?;
    for extension in &extensions {
        let path = package_path(extension);
        ensure!(path.is_file(), "{} has not been built", path.display());
        validate_package(extension, &path)?;
    }
    println!("validated {} .manatan2 packages", extensions.len());
    Ok(())
}

fn validate_package(extension: &ExtensionDir, path: &Path) -> Result<()> {
    let mut archive = ZipArchive::new(File::open(path)?)?;
    let mut actual = BTreeSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        ensure!(
            !file.is_dir(),
            "{} contains a directory entry",
            path.display()
        );
        ensure!(
            actual.insert(file.name().to_owned()),
            "{} has duplicate entries",
            path.display()
        );
    }
    let mut expected =
        BTreeSet::from(["manifest.json".to_owned(), extension.manifest.wasm.clone()]);
    expected.extend(
        extension
            .manifest
            .assets
            .iter()
            .map(|asset| asset.path.clone()),
    );
    ensure!(
        actual == expected,
        "{} contains undeclared or missing files",
        path.display()
    );
    let mut manifest_bytes = Vec::new();
    archive
        .by_name("manifest.json")?
        .read_to_end(&mut manifest_bytes)?;
    let packaged_manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;
    ensure!(
        packaged_manifest == extension.manifest,
        "{} manifest is stale",
        path.display()
    );
    for asset in &extension.manifest.assets {
        let mut bytes = Vec::new();
        archive.by_name(&asset.path)?.read_to_end(&mut bytes)?;
        ensure!(
            asset.sha256.as_deref() == Some(sha256(&bytes).as_str()),
            "{} asset digest mismatch",
            asset.path
        );
    }
    let mut component = Vec::new();
    archive
        .by_name(&extension.manifest.wasm)?
        .read_to_end(&mut component)?;
    let component_path = root().join("target/manatan2-components/package-validation.wasm");
    if let Some(parent) = component_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&component_path, component)?;
    run(
        Command::new("wasm-tools")
            .args(["validate", "--features", "component-model"])
            .arg(&component_path),
        "validate packaged component",
    )?;
    Ok(())
}

fn generate_indexes() -> Result<()> {
    let extensions = discover_extensions()?;
    let matrix: PortingMatrix =
        serde_json::from_slice(&fs::read(root().join("porting-matrix.json"))?)?;
    let statuses = matrix
        .sources
        .iter()
        .map(|row| {
            (
                (
                    row.media_kind.as_str(),
                    row.language.as_str(),
                    row.source_id.as_str(),
                ),
                row.status.as_str(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut catalog = Vec::new();
    let mut summaries = Vec::new();
    for media in MEDIA {
        let mut entries = Vec::new();
        for extension in extensions
            .iter()
            .filter(|extension| extension.media == media)
        {
            let package = package_path(extension);
            ensure!(
                package.is_file(),
                "build {} before generating indexes",
                extension.id
            );
            let package_bytes = fs::read(&package)?;
            let package_relative = format!(
                "packages/{}/{}/{}.manatan2",
                extension.media, extension.lang, extension.manifest.id
            );
            let icon_url = if let Some(icon) = extension.manifest.icon.as_ref() {
                let relative = format!(
                    "icons/{}/{}/{}.png",
                    extension.media, extension.lang, extension.id
                );
                let output = root().join("dist").join(&relative);
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(extension.path.join(icon), &output)?;
                Some(relative)
            } else {
                None
            };
            let first_source = extension.manifest.sources.first().context("source")?;
            let source_ids = extension
                .manifest
                .sources
                .iter()
                .map(|source| source.id.clone())
                .collect::<Vec<_>>();
            let source_names = extension
                .manifest
                .sources
                .iter()
                .map(|source| source.name.clone())
                .collect::<Vec<_>>();
            let verified = statuses
                .get(&(
                    extension.media.as_str(),
                    extension.lang.as_str(),
                    first_source.id.as_str(),
                ))
                .is_some_and(|status| matches!(*status, "runtime-tested" | "live-verified"));
            let entry = json!({
                "schemaVersion": 2,
                "pkgName": format!("manatan:{}", extension.manifest.id),
                "id": extension.manifest.id,
                "packageId": extension.manifest.id,
                "name": extension.manifest.name,
                "versionName": extension.manifest.version,
                "version": extension.manifest.version,
                "versionCode": extension.manifest.version_code,
                "contentType": media,
                "media": media,
                "mediaKind": media,
                "lang": first_source.lang,
                "language": first_source.lang,
                "contentRating": first_source.content_rating,
                "extensionType": "manatan2",
                "packageUrl": package_relative,
                "packagePath": package_relative,
                "sha256": sha256(&package_bytes),
                "size": package_bytes.len(),
                "sizeBytes": package_bytes.len(),
                "iconUrl": icon_url,
                "sourceIds": source_ids,
                "sourceNames": source_names,
                "verified": verified,
                "sources": extension.manifest.sources,
                "manifest": extension.manifest,
            });
            if verified {
                entries.push(entry.clone());
            }
            catalog.push(entry);
        }
        write_json_pair(&root().join("dist"), media, &entries)?;
        summaries.push(json!({
            "media": media,
            "index": format!("{media}.min.json"),
            "count": entries.len(),
            "verifiedOnly": true,
        }));
    }
    catalog.sort_by(|left, right| {
        left["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["name"].as_str().unwrap_or_default())
    });
    write_json_pair(&root().join("dist"), "catalog", &catalog)?;
    fs::write(
        root().join("dist/index.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 2,
            "apiVersion": 2,
            "extensionType": "manatan2",
            "indexes": summaries,
        }))?,
    )?;
    println!("generated indexes for {} packages", catalog.len());
    Ok(())
}

fn publish(destination: &str) -> Result<()> {
    generate_indexes()?;
    validate_packages()?;
    let destination = PathBuf::from(destination).canonicalize()?;
    ensure!(
        destination.join(".git").is_dir(),
        "{} is not a generated repository",
        destination.display()
    );
    for path in [
        destination.join("packages"),
        destination.join("icons"),
        destination.join("docs/packages"),
        destination.join("docs/icons"),
    ] {
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
    }
    let docs_destination = destination.join("docs");
    for pattern in [
        "manga.json",
        "manga.min.json",
        "manga.preview.json",
        "manga.preview.min.json",
        "video.json",
        "video.min.json",
        "video.preview.json",
        "video.preview.min.json",
        "novel.json",
        "novel.min.json",
        "novel.preview.json",
        "novel.preview.min.json",
        "catalog.json",
        "catalog.min.json",
        "index.json",
        "verification.json",
    ] {
        for base in [&destination, &docs_destination] {
            let path = base.join(pattern);
            if path.is_file() {
                fs::remove_file(path)?;
            }
        }
    }
    for directory in ["packages", "icons"] {
        copy_tree(
            &root().join("dist").join(directory),
            &destination.join(directory),
        )?;
        copy_tree(
            &root().join("dist").join(directory),
            &destination.join("docs").join(directory),
        )?;
    }
    for name in [
        "manga.json",
        "manga.min.json",
        "video.json",
        "video.min.json",
        "novel.json",
        "novel.min.json",
        "catalog.json",
        "catalog.min.json",
        "index.json",
    ] {
        let source = root().join("dist").join(name);
        fs::copy(&source, destination.join(name))?;
        fs::copy(&source, destination.join("docs").join(name))?;
    }
    println!("published .manatan2 outputs to {}", destination.display());
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(source) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        let output = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&output)?;
        } else {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), output)?;
        }
    }
    Ok(())
}

fn write_json_pair(directory: &Path, name: &str, value: &[Value]) -> Result<()> {
    fs::create_dir_all(directory)?;
    fs::write(
        directory.join(format!("{name}.json")),
        serde_json::to_vec_pretty(value)?,
    )?;
    fs::write(
        directory.join(format!("{name}.min.json")),
        serde_json::to_vec(value)?,
    )?;
    Ok(())
}

fn runtime_test(
    relative: &str,
    runtime_argument: Option<&str>,
    operation: Option<&str>,
    request_json: Option<&str>,
) -> Result<()> {
    let extension = discover_extensions()?
        .into_iter()
        .find(|extension| {
            format!("{}/{}/{}", extension.media, extension.lang, extension.id) == relative
        })
        .with_context(|| format!("unknown extension {relative}"))?;
    let package = package_path(&extension);
    ensure!(
        package.is_file(),
        "build {} before runtime testing",
        extension.id
    );
    let runtime = runtime_argument
        .map(PathBuf::from)
        .or_else(|| env::var_os("MANATAN_RUNTIME_ROOT").map(PathBuf::from))
        .context("pass Manatan-Private2 path or set MANATAN_RUNTIME_ROOT")?
        .canonicalize()?;
    let harness = root().join("target/manatan2-runtime-smoke");
    fs::create_dir_all(harness.join("src"))?;
    let runtime_crate = runtime.join("crates/manatan-extension");
    fs::write(
        harness.join("Cargo.toml"),
        format!(
            "[workspace]\n\n[package]\nname = \"manatan2-runtime-smoke\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nmanatan-extension = {{ path = {:?}, features = [\"archive\", \"runner\"] }}\nserde_json = \"1\"\n",
            runtime_crate
        ),
    )?;
    fs::write(
        harness.join("src/main.rs"),
        r#"use manatan_extension::{parse_archive, runner::ExtensionRunner};
use serde_json::json;

fn main() {
    let mut args = std::env::args().skip(1);
    let package = std::fs::read(args.next().expect("package path")).expect("read package");
    let operation = args.next().unwrap_or_else(|| "filters".to_owned());
    let archive = parse_archive(&package).expect("parse .manatan2 package");
    let source_id = archive.manifest.sources.first().expect("source").id.clone();
    let mut request = args
        .next()
        .map(|value| serde_json::from_str(&value).expect("parse request JSON"))
        .unwrap_or_else(|| json!({}));
    request
        .as_object_mut()
        .expect("request JSON must be an object")
        .insert("sourceId".to_owned(), json!(source_id));
    let runner = ExtensionRunner::new(archive);
    let value = runner.call_value(&operation, request).expect("run component operation");
    if operation == "filters" {
        assert!(value.is_array(), "filters export must return an array");
    }
    println!("{}", serde_json::to_string(&value).expect("serialize result"));
}
"#,
    )?;
    run(
        Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(&harness)
            .args(["run", "--quiet", "--"])
            .arg(package)
            .arg(operation.unwrap_or("filters"))
            .args(request_json),
        "execute component through production Wasmtime runner",
    )?;
    println!("runtime-tested {relative}");
    Ok(())
}

fn package_path(extension: &ExtensionDir) -> PathBuf {
    root()
        .join("dist/packages")
        .join(&extension.media)
        .join(&extension.lang)
        .join(format!("{}.manatan2", extension.manifest.id))
}

fn content_type_name(content_type: ContentType) -> &'static str {
    match content_type {
        ContentType::Manga => "manga",
        ContentType::Video => "video",
        ContentType::Novel => "novel",
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
