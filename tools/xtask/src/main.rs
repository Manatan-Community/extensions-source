use anyhow::{Context, Result, anyhow, bail};
use manatan_extension::{
    abi::{ExtensionError, HttpRequest, HttpResponse},
    parse_archive,
    runner::{EmptyHost, ExtensionRunner, HostCall, RunnerError},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File},
    io::{Read, Seek},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use zip::{
    CompressionMethod, DateTime, ZipArchive, ZipWriter,
    write::{FileOptions, SimpleFileOptions},
};

const MEDIA_TYPES: [&str; 3] = ["manga", "video", "novel"];
const ALLOWED_ROOT_DIRS: [&str; 8] = [
    "manga", "video", "novel", "shared", "tools", "docs", "dist", "target",
];

#[derive(Debug, Clone)]
struct Extension {
    media: String,
    lang: String,
    slug: String,
    dir: PathBuf,
    manifest: Manifest,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    package_id: String,
    name: String,
    version: String,
    version_code: u64,
    lang: String,
    content_type: String,
    #[serde(default)]
    sources: Vec<Source>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    content_rating: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Source {
    id: String,
    name: String,
    lang: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerificationRegistry {
    #[serde(default)]
    verified: BTreeMap<String, VerificationRecord>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerificationRecord {
    verified_at: String,
    verified_by: String,
    #[serde(default)]
    checks: Vec<String>,
    #[serde(default)]
    notes: Option<String>,
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("validate") => validate(),
        Some("test-examples") => test_examples(),
        Some("build-extension") => {
            let path = args.next().ok_or_else(|| {
                anyhow!("usage: cargo run -p xtask -- build-extension <media/lang/id>")
            })?;
            let ext = find_extensions()?
                .into_iter()
                .find(|ext| ext.rel_path() == path)
                .ok_or_else(|| anyhow!("extension not found: {path}"))?;
            package_extension(&ext, true).map(|_| ())
        }
        Some("build-all") => {
            for ext in find_extensions()? {
                println!("Building {}", ext.rel_path());
                package_extension(&ext, true)?;
            }
            Ok(())
        }
        Some("generate-index") => generate_index(),
        Some("inventory-upstreams") => inventory_upstreams(),
        Some("validate-packages") => validate_packages(),
        Some("smoke-test") => smoke_test(args.next()),
        _ => bail!(
            "usage: cargo run -p xtask -- <validate|test-examples|build-extension|build-all|generate-index|inventory-upstreams|validate-packages|smoke-test>"
        ),
    }
}

impl Extension {
    fn rel_path(&self) -> String {
        format!("{}/{}/{}", self.media, self.lang, self.slug)
    }

    fn package_rel_path(&self) -> String {
        format!(
            "{}/{}/{}.manatan",
            self.media, self.lang, self.manifest.package_id
        )
    }
}

fn repo_root() -> Result<PathBuf> {
    Ok(env::current_dir()?.canonicalize()?)
}

fn dist_root() -> Result<PathBuf> {
    Ok(repo_root()?.join("dist"))
}

fn verification_registry_path() -> Result<PathBuf> {
    Ok(repo_root()?.join("verification.json"))
}

fn load_verification_registry() -> Result<VerificationRegistry> {
    let path = verification_registry_path()?;
    if !path.exists() {
        return Ok(VerificationRegistry::default());
    }
    serde_json::from_reader(File::open(&path)?)
        .with_context(|| format!("invalid verification registry: {}", path.display()))
}

fn find_extensions() -> Result<Vec<Extension>> {
    let root = repo_root()?;
    let mut out = Vec::new();
    for media in MEDIA_TYPES {
        for lang_dir in read_dirs(&root.join(media))? {
            for ext_dir in read_dirs(&lang_dir)? {
                let manifest_path = ext_dir.join("manifest.json");
                if !manifest_path.exists() {
                    continue;
                }
                let manifest: Manifest = serde_json::from_reader(File::open(&manifest_path)?)
                    .with_context(|| format!("invalid JSON: {}", manifest_path.display()))?;
                out.push(Extension {
                    media: media.to_string(),
                    lang: lang_dir.file_name_string()?,
                    slug: ext_dir.file_name_string()?,
                    dir: ext_dir,
                    manifest,
                });
            }
        }
    }
    out.sort_by(|a, b| a.rel_path().cmp(&b.rel_path()));
    Ok(out)
}

fn read_dirs(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("reading {}", path.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

trait FileNameString {
    fn file_name_string(&self) -> Result<String>;
}

impl FileNameString for PathBuf {
    fn file_name_string(&self) -> Result<String> {
        Ok(self
            .file_name()
            .context("missing file name")?
            .to_string_lossy()
            .into_owned())
    }
}

fn validate() -> Result<()> {
    let root = repo_root()?;
    let mut errors = Vec::new();

    for media in MEDIA_TYPES {
        if !root.join(media).is_dir() {
            errors.push(format!("missing root folder: {media}/"));
        }
    }
    for required in ["shared", "tools"] {
        if !root.join(required).is_dir() {
            errors.push(format!("missing root folder: {required}/"));
        }
    }
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || ALLOWED_ROOT_DIRS.contains(&name.as_str()) {
            continue;
        }
        errors.push(format!("unexpected root folder: {name}/"));
    }

    let extensions = find_extensions()?;
    let verification = load_verification_registry()?;
    let extension_paths = extensions
        .iter()
        .map(Extension::rel_path)
        .collect::<BTreeSet<_>>();

    for key in verification.verified.keys() {
        if !extension_paths.contains(key) {
            errors.push(format!(
                "verification.json: verified extension does not exist: {key}"
            ));
        }
    }

    let mut package_ids = BTreeMap::<String, String>::new();
    let mut source_ids = BTreeMap::<String, String>::new();

    for ext in &extensions {
        let manifest_file = ext.dir.join("manifest.json");
        let manifest = &ext.manifest;
        let location = ext.rel_path();

        if manifest.package_id != ext.slug {
            errors.push(format!(
                "{}: packageId must match folder id {:?}",
                manifest_file.display(),
                ext.slug
            ));
        }
        if manifest.schema_version != 1 {
            errors.push(format!(
                "{}: schemaVersion must be 1",
                manifest_file.display()
            ));
        }
        if !is_slug(&manifest.package_id) {
            errors.push(format!(
                "{}: packageId must be lowercase letters, digits, and hyphens",
                manifest_file.display()
            ));
        }
        let package_key = format!("{}/{}/{}", ext.media, ext.lang, manifest.package_id);
        if let Some(previous) = package_ids.insert(package_key, location.clone()) {
            errors.push(format!(
                "{}: duplicate packageId {:?}, already used by {}",
                manifest_file.display(),
                manifest.package_id,
                previous
            ));
        }
        if manifest.name.trim().is_empty() {
            errors.push(format!("{}: name is required", manifest_file.display()));
        }
        if manifest.version.trim().is_empty() {
            errors.push(format!("{}: version is required", manifest_file.display()));
        }
        if manifest.version_code == 0 {
            errors.push(format!(
                "{}: versionCode must be positive",
                manifest_file.display()
            ));
        }
        if !lang_matches_folder(&manifest.lang, &ext.lang) {
            errors.push(format!(
                "{}: lang must match folder lang {:?}",
                manifest_file.display(),
                ext.lang
            ));
        }
        if manifest.content_type != ext.media {
            errors.push(format!(
                "{}: contentType must be {:?}",
                manifest_file.display(),
                ext.media
            ));
        }
        if !["safe", "suggestive", "adult"]
            .contains(&manifest.content_rating.as_deref().unwrap_or("safe"))
        {
            errors.push(format!(
                "{}: contentRating must be safe, suggestive, or adult",
                manifest_file.display()
            ));
        }
        if manifest.sources.is_empty() {
            errors.push(format!("{}: sources is required", manifest_file.display()));
        }
        for source in &manifest.sources {
            if !is_slug(&source.id) {
                errors.push(format!(
                    "{}: source id {:?} must be a lowercase slug",
                    manifest_file.display(),
                    source.id
                ));
            }
            if let Some(previous) = source_ids.insert(source.id.clone(), location.clone()) {
                errors.push(format!(
                    "{}: duplicate source id {:?}, already used by {}",
                    manifest_file.display(),
                    source.id,
                    previous
                ));
            }
            if !source
                .lang
                .as_deref()
                .is_some_and(|lang| lang_matches_folder(lang, &ext.lang))
                && ext.lang != "all"
            {
                errors.push(format!(
                    "{}: source {:?} lang must match folder lang {:?}, unless the package folder is all/",
                    manifest_file.display(),
                    source.id,
                    ext.lang
                ));
            }
            if source.content_type.as_deref() != Some(&ext.media) {
                errors.push(format!(
                    "{}: source {:?} contentType must match package media {:?}",
                    manifest_file.display(),
                    source.id,
                    ext.media
                ));
            }
        }
        if let Some(icon) = &manifest.icon {
            if icon.starts_with('/') || icon.contains("..") || icon.trim().is_empty() {
                errors.push(format!(
                    "{}: icon must be a relative path inside the extension",
                    manifest_file.display()
                ));
            } else if !ext.dir.join(icon).is_file() {
                errors.push(format!(
                    "{}: declared icon is missing: {}",
                    manifest_file.display(),
                    ext.dir.join(icon).display()
                ));
            }
        }

        let cargo = ext.dir.join("Cargo.toml");
        let lib = ext.dir.join("src/lib.rs");
        if !cargo.is_file() {
            errors.push(format!("{}: missing Cargo.toml", ext.dir.display()));
        }
        if !lib.is_file() {
            errors.push(format!("{}: missing src/lib.rs", ext.dir.display()));
        }
        let cargo_text = fs::read_to_string(&cargo).unwrap_or_default();
        if !cargo_text.contains("cdylib") {
            errors.push(format!(
                "{}: crate-type must include cdylib",
                cargo.display()
            ));
        }
        if !uses_manatan_sdk(&cargo_text) {
            errors.push(format!(
                "{}: must use the Manatan Rust SDK Git URL or local SDK path",
                cargo.display()
            ));
        }
    }

    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    println!("Validated {} extension(s).", extensions.len());
    Ok(())
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && value
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
}

fn uses_manatan_sdk(cargo_text: &str) -> bool {
    cargo_text.contains("https://github.com/KolbyML/manatan-rs")
        || (cargo_text.contains("manatan-extension")
            && cargo_text.contains("path")
            && cargo_text.contains("manatan-rs"))
}

fn lang_matches_folder(value: &str, folder_lang: &str) -> bool {
    value == folder_lang
        || value
            .strip_prefix(folder_lang)
            .is_some_and(|rest| rest.starts_with('-'))
}

fn manatan_slug(value: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn test_examples() -> Result<()> {
    for ext in find_extensions()? {
        println!("Testing {}", ext.rel_path());
        let status = Command::new("cargo")
            .arg("test")
            .current_dir(&ext.dir)
            .status()
            .with_context(|| format!("running cargo test in {}", ext.dir.display()))?;
        if !status.success() {
            bail!("cargo test failed for {}", ext.rel_path());
        }
    }
    Ok(())
}

fn package_extension(ext: &Extension, build: bool) -> Result<PathBuf> {
    let target_dir = extension_target_dir(ext)?;
    if build {
        let status = Command::new("cargo")
            .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
            .env("CARGO_TARGET_DIR", &target_dir)
            .current_dir(&ext.dir)
            .status()
            .with_context(|| format!("running cargo build in {}", ext.dir.display()))?;
        if !status.success() {
            bail!("cargo build failed for {}", ext.rel_path());
        }
    }

    let wasm_name = format!("{}.wasm", ext.slug.replace('-', "_"));
    let wasm = target_dir
        .join("wasm32-unknown-unknown/release")
        .join(wasm_name);
    if !wasm.exists() {
        bail!("missing built wasm: {}", wasm.display());
    }

    let out_file = dist_root()?.join(ext.package_rel_path());
    fs::create_dir_all(out_file.parent().context("package has no parent")?)?;
    let file = File::create(&out_file)?;
    let mut zip = ZipWriter::new(file);
    let options = deterministic_zip_options()?;

    add_file(
        &mut zip,
        &ext.dir.join("manifest.json"),
        "manifest.json",
        options,
    )?;
    for optional in ["filters.json", "preferences.json"] {
        let path = ext.dir.join(optional);
        if path.exists() {
            add_file(&mut zip, &path, optional, options)?;
        }
    }
    let assets = ext.dir.join("assets");
    if assets.exists() {
        for file in sorted_files(&assets)? {
            let rel = file
                .strip_prefix(&assets)?
                .to_string_lossy()
                .replace('\\', "/");
            add_file(&mut zip, &file, &format!("assets/{rel}"), options)?;
        }
    }
    add_file(&mut zip, &wasm, "module.wasm", options)?;
    zip.finish()?;
    println!("{}", out_file.display());
    Ok(out_file)
}

fn extension_target_dir(ext: &Extension) -> Result<PathBuf> {
    Ok(env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                ext.dir.join(path)
            }
        })
        .unwrap_or(repo_root()?.join("target/extension-builds")))
}

fn deterministic_zip_options() -> Result<SimpleFileOptions> {
    Ok(FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)?))
}

fn sorted_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_files(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn add_file(
    zip: &mut ZipWriter<File>,
    path: &Path,
    name: &str,
    options: SimpleFileOptions,
) -> Result<()> {
    zip.start_file(name, options)?;
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    std::io::copy(&mut file, zip)?;
    Ok(())
}

fn generate_index() -> Result<()> {
    validate()?;
    let base_url = env::var("MANATAN_EXTENSIONS_BASE_URL").unwrap_or_else(|_| ".".to_string());
    let dist = dist_root()?;
    fs::create_dir_all(&dist)?;
    let extensions = find_extensions()?;
    let verification = load_verification_registry()?;
    let mut catalog = Vec::new();

    for media in MEDIA_TYPES {
        let mut entries = Vec::new();
        let mut preview_entries = Vec::new();
        for ext in extensions.iter().filter(|ext| ext.media == media) {
            let verification_record = verification.verified.get(&ext.rel_path());
            let entry = index_entry(ext, &dist, &base_url, verification_record)?;
            catalog.push(entry.clone());
            preview_entries.push(entry.clone());
            if verification_record.is_some() {
                entries.push(entry);
            }
        }
        sort_index_entries_by_id(&mut entries);
        sort_index_entries_by_id(&mut preview_entries);
        fs::write(
            dist.join(format!("{media}.min.json")),
            serde_json::to_vec(&entries)?,
        )?;
        fs::write(
            dist.join(format!("{media}.preview.min.json")),
            serde_json::to_vec(&preview_entries)?,
        )?;
    }
    catalog.sort_by(|a, b| {
        let media_cmp = a
            .get("media")
            .and_then(Value::as_str)
            .cmp(&b.get("media").and_then(Value::as_str));
        if media_cmp.is_eq() {
            a.get("id")
                .and_then(Value::as_str)
                .cmp(&b.get("id").and_then(Value::as_str))
        } else {
            media_cmp
        }
    });
    fs::write(dist.join("catalog.min.json"), serde_json::to_vec(&catalog)?)?;
    Ok(())
}

fn sort_index_entries_by_id(entries: &mut [Value]) {
    entries.sort_by(|a, b| {
        a.get("id")
            .and_then(Value::as_str)
            .cmp(&b.get("id").and_then(Value::as_str))
    });
}

fn index_entry(
    ext: &Extension,
    dist: &Path,
    base_url: &str,
    verification: Option<&VerificationRecord>,
) -> Result<Value> {
    let package_path = ext.package_rel_path();
    let package = dist.join(&package_path);
    let metadata =
        fs::metadata(&package).with_context(|| format!("missing package {}", package.display()))?;
    let (icon_path, icon_url) = if let Some(icon) = &ext.manifest.icon {
        let rel = format!("icons/{}/{}", ext.manifest.package_id, icon);
        let dest = dist.join(&rel);
        fs::create_dir_all(dest.parent().context("icon has no parent")?)?;
        fs::copy(ext.dir.join(icon), &dest)?;
        (Some(rel.clone()), Some(rel))
    } else {
        (None, None)
    };
    Ok(json!({
        "id": ext.manifest.package_id,
        "packageId": ext.manifest.package_id,
        "name": ext.manifest.name,
        "media": ext.media,
        "mediaKind": ext.manifest.content_type,
        "lang": ext.manifest.lang,
        "language": ext.manifest.lang,
        "contentRating": ext.manifest.content_rating.as_deref().unwrap_or("safe"),
        "version": ext.manifest.version,
        "versionName": ext.manifest.version,
        "versionCode": ext.manifest.version_code,
        "packagePath": package_path,
        "packageUrl": join_base(base_url, &ext.package_rel_path()),
        "sha256": sha256_file(&package)?,
        "sizeBytes": metadata.len(),
        "iconPath": icon_path,
        "iconUrl": icon_url,
        "sourceIds": ext.manifest.sources.iter().map(|source| source.id.clone()).collect::<Vec<_>>(),
        "verified": verification.is_some(),
        "verification": verification
    }))
}

fn join_base(base: &str, path: &str) -> String {
    if base == "." {
        path.to_string()
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn validate_packages() -> Result<()> {
    let extensions = find_extensions()?;
    for ext in &extensions {
        let package = dist_root()?.join(ext.package_rel_path());
        let bytes = fs::read(&package).with_context(|| format!("reading {}", package.display()))?;
        let archive =
            parse_archive(&bytes).with_context(|| format!("parsing {}", package.display()))?;
        let archive_media = serde_json::to_string(&archive.manifest.content_type)?
            .trim_matches('"')
            .to_string();
        if archive_media != ext.media {
            bail!(
                "{}: package contentType {:?} does not match folder {:?}",
                package.display(),
                archive_media,
                ext.media
            );
        }
        inspect_zip(&package, ext)?;
    }
    println!("Validated {} package(s).", extensions.len());
    Ok(())
}

fn inspect_zip(path: &Path, ext: &Extension) -> Result<()> {
    let file = File::open(path)?;
    let mut zip = ZipArchive::new(file)?;
    require_zip_file(&mut zip, "manifest.json", path)?;
    require_zip_file(&mut zip, "module.wasm", path)?;
    if let Some(icon) = &ext.manifest.icon {
        require_zip_file(&mut zip, icon, path)?;
    }
    if path.extension().and_then(|value| value.to_str()) != Some("manatan") {
        bail!("{}: package extension must be .manatan", path.display());
    }
    Ok(())
}

fn require_zip_file<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
    path: &Path,
) -> Result<()> {
    zip.by_name(name)
        .map(|_| ())
        .with_context(|| format!("{}: missing {name}", path.display()))
}

fn smoke_test(target: Option<String>) -> Result<()> {
    let mut extensions = find_extensions()?;
    if let Some(target) = target {
        let prefix = format!("{}/", target.trim_end_matches('/'));
        extensions.retain(|ext| {
            let rel = ext.rel_path();
            rel == target || rel.starts_with(&prefix)
        });
        if extensions.is_empty() {
            bail!("extension not found: {target}");
        }
    }
    for ext in &extensions {
        let package = dist_root()?.join(ext.package_rel_path());
        let bytes = fs::read(&package).with_context(|| format!("reading {}", package.display()))?;
        let archive =
            parse_archive(&bytes).with_context(|| format!("parsing {}", package.display()))?;
        let runner = ExtensionRunner::with_host(archive, Arc::new(SmokeHost));
        let export = match ext.media.as_str() {
            "manga" => "manatan_manga_get_list",
            "video" => "manatan_video_get_list",
            "novel" => "manatan_novel_get_list",
            _ => bail!("unsupported media {}", ext.media),
        };
        println!("Smoke-testing {}", ext.dir.display());
        let _: Value = runner
            .call_value(export, json!({}))
            .with_context(|| format!("calling {export} in {}", package.display()))?;
    }
    println!("Smoke-tested {} package(s).", extensions.len());
    Ok(())
}

#[derive(Default)]
struct SmokeHost;

impl HostCall for SmokeHost {
    fn call(&self, operation: &str, payload: &[u8]) -> std::result::Result<Vec<u8>, RunnerError> {
        match operation {
            "http.fetch" => smoke_http_fetch(payload),
            "cookies.get" | "cookies.set" => smoke_host_ok(json!({
                "header": null,
                "cookies": []
            })),
            "storage.get" | "storage.set" | "storage.delete" | "storage.list" => {
                smoke_host_ok(json!({
                    "value": null,
                    "entries": []
                }))
            }
            "system.time" => {
                let unix_millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| RunnerError::Host(format!("system time error: {error}")))?
                    .as_millis() as i64;
                smoke_host_ok(json!({
                    "unixMillis": unix_millis,
                    "unixSeconds": unix_millis / 1_000
                }))
            }
            _ => EmptyHost.call(operation, payload),
        }
    }
}

fn smoke_http_fetch(payload: &[u8]) -> std::result::Result<Vec<u8>, RunnerError> {
    let request: HttpRequest = serde_json::from_slice(payload).map_err(RunnerError::from)?;
    if env::var("MANATAN_SMOKE_LIVE_HTTP").ok().as_deref() != Some("1") {
        return host_err("live HTTP is disabled during smoke tests");
    }
    if request.body_base64.is_some() {
        return host_err("smoke HTTP host does not support request bodies yet");
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RunnerError::Host(format!("system time error: {error}")))?
        .as_millis();
    let temp = env::temp_dir();
    let headers_path = temp.join(format!(
        "manatan-smoke-{}-{stamp}.headers",
        std::process::id()
    ));
    let body_path = temp.join(format!("manatan-smoke-{}-{stamp}.body", std::process::id()));

    let mut command = Command::new("curl");
    command.args([
        "--location",
        "--silent",
        "--show-error",
        "--max-time",
        "25",
        "--dump-header",
    ]);
    command.arg(&headers_path);
    command.args(["--output"]);
    command.arg(&body_path);
    command.args(["--write-out", "%{http_code}\n%{url_effective}", "--request"]);
    command.arg(&request.method);
    for (name, value) in &request.headers {
        command.arg("--header");
        command.arg(format!("{name}: {value}"));
    }
    command.arg(&request.url);

    let output = command
        .output()
        .map_err(|error| RunnerError::Host(format!("running curl: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = fs::remove_file(&headers_path);
        let _ = fs::remove_file(&body_path);
        return host_err(&format!("curl failed for {}: {stderr}", request.url));
    }

    let write_out = String::from_utf8_lossy(&output.stdout);
    let mut lines = write_out.lines();
    let status = lines
        .next()
        .and_then(|line| line.parse::<u16>().ok())
        .unwrap_or(0);
    let final_url = lines.next().unwrap_or(request.url.as_str()).to_string();
    let headers = parse_curl_headers(&fs::read_to_string(&headers_path).unwrap_or_default());
    let body = fs::read(&body_path).unwrap_or_default();
    let _ = fs::remove_file(&headers_path);
    let _ = fs::remove_file(&body_path);

    let response = HttpResponse {
        status,
        headers,
        final_url,
        body_base64: None,
        text: Some(String::from_utf8_lossy(&body).into_owned()),
    };
    serde_json::to_vec(&Ok::<HttpResponse, ExtensionError>(response)).map_err(RunnerError::from)
}

fn parse_curl_headers(input: &str) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for line in input.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("HTTP/") {
            headers.clear();
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    headers
}

fn host_err(message: &str) -> std::result::Result<Vec<u8>, RunnerError> {
    serde_json::to_vec(&Err::<Value, _>(ExtensionError {
        message: message.to_string(),
    }))
    .map_err(RunnerError::from)
}

fn smoke_host_ok(value: Value) -> std::result::Result<Vec<u8>, RunnerError> {
    serde_json::to_vec(&Ok::<Value, ExtensionError>(value)).map_err(RunnerError::from)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Debug, Clone)]
struct InventoryRow {
    media: &'static str,
    upstream_path: PathBuf,
    target_path: PathBuf,
    status: &'static str,
    notes: String,
}

fn inventory_upstreams() -> Result<()> {
    let root = repo_root()?;
    let repo_parent = root.parent().context("repo root has no parent")?;
    let mut rows = Vec::new();

    rows.extend(inventory_gradle_sources(
        "video",
        &repo_parent.join("upstream-anime-extensions/src"),
        &root.join("video"),
    )?);
    rows.extend(inventory_gradle_sources(
        "manga",
        &repo_parent.join("upstream-keiyoushi-extensions-source/src"),
        &root.join("manga"),
    )?);
    rows.extend(inventory_novel_sources(
        &repo_parent.join("upstream-lnreader-plugins/plugins"),
        &root.join("novel"),
    )?);
    apply_inventory_overrides(&root, &mut rows)?;

    rows.sort_by(|a, b| {
        a.media
            .cmp(b.media)
            .then_with(|| a.target_path.cmp(&b.target_path))
            .then_with(|| a.upstream_path.cmp(&b.upstream_path))
    });

    fs::create_dir_all(root.join("docs"))?;
    fs::write(
        root.join("docs/upstream-inventory.md"),
        render_inventory(&root, &rows),
    )?;
    println!(
        "Wrote docs/upstream-inventory.md with {} row(s).",
        rows.len()
    );
    Ok(())
}

fn inventory_gradle_sources(
    media: &'static str,
    src_root: &Path,
    target_root: &Path,
) -> Result<Vec<InventoryRow>> {
    let mut rows = Vec::new();
    if !src_root.exists() {
        return Ok(rows);
    }
    for lang_dir in read_dirs(src_root)? {
        for source_dir in read_dirs(&lang_dir)? {
            let build_gradle = source_dir.join("build.gradle");
            if !build_gradle.exists() {
                continue;
            }
            let lang = lang_dir.file_name_string()?;
            let slug = manatan_slug(&source_dir.file_name_string()?);
            let target = target_root.join(&lang).join(&slug);
            let status = if target.join("manifest.json").exists() {
                "ported"
            } else {
                "not-started"
            };
            rows.push(InventoryRow {
                media,
                upstream_path: source_dir,
                target_path: target,
                status,
                notes: gradle_notes(&build_gradle)?,
            });
        }
    }
    Ok(rows)
}

fn gradle_notes(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut notes = Vec::new();
    if text.contains("multisrc") || text.contains("lib-multisrc") {
        notes.push("shared source family");
    }
    if text.contains("WebView") || text.contains("webview") {
        notes.push("check webview flow");
    }
    if notes.is_empty() {
        Ok(String::new())
    } else {
        Ok(notes.join("; "))
    }
}

fn inventory_novel_sources(plugins_root: &Path, target_root: &Path) -> Result<Vec<InventoryRow>> {
    let mut rows = Vec::new();
    if !plugins_root.exists() {
        return Ok(rows);
    }
    for lang_dir in read_dirs(plugins_root)? {
        let language = lang_dir.file_name_string()?;
        for entry in fs::read_dir(&lang_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("ts") {
                continue;
            }
            let file_name = path.file_name_string()?;
            let broken = file_name.ends_with(".broken.ts");
            let slug = file_name
                .trim_end_matches(".broken.ts")
                .trim_end_matches(".ts")
                .replace('_', "-")
                .to_ascii_lowercase();
            let lang = language_code(&language);
            let target = target_root.join(lang).join(&slug);
            rows.push(InventoryRow {
                media: "novel",
                upstream_path: path,
                target_path: target.clone(),
                status: if target.join("manifest.json").exists() {
                    "ported"
                } else if broken {
                    "blocked"
                } else {
                    "not-started"
                },
                notes: if broken {
                    "upstream source is marked broken".to_string()
                } else {
                    String::new()
                },
            });
        }
    }
    Ok(rows)
}

#[derive(Debug, Deserialize)]
struct InventoryOverride {
    target: String,
    #[serde(default, rename = "localTarget")]
    local_target: Option<String>,
    status: String,
    notes: String,
}

fn apply_inventory_overrides(root: &Path, rows: &mut [InventoryRow]) -> Result<()> {
    let path = root.join("docs/upstream-overrides.json");
    if !path.exists() {
        return Ok(());
    }
    let overrides: Vec<InventoryOverride> =
        serde_json::from_reader(File::open(&path)?).with_context(|| path.display().to_string())?;
    for override_row in overrides {
        if !["not-started", "ported", "blocked", "unsupported"]
            .contains(&override_row.status.as_str())
        {
            bail!(
                "{}: invalid inventory status {}",
                path.display(),
                override_row.status
            );
        }
        for row in rows.iter_mut() {
            let target = row
                .target_path
                .strip_prefix(root)
                .unwrap_or(&row.target_path);
            if target.to_string_lossy() == override_row.target {
                if let Some(local_target) = &override_row.local_target {
                    row.target_path = root.join(local_target);
                }
                row.status = match override_row.status.as_str() {
                    "not-started" => "not-started",
                    "ported" => "ported",
                    "blocked" => "blocked",
                    "unsupported" => "unsupported",
                    _ => unreachable!(),
                };
                row.notes = override_row.notes.clone();
            }
        }
    }
    Ok(())
}

fn language_code(language: &str) -> &'static str {
    match language.to_ascii_lowercase().as_str() {
        "arabic" => "ar",
        "chinese" => "zh",
        "english" => "en",
        "french" => "fr",
        "indonesian" => "id",
        "japanese" => "ja",
        "korean" => "ko",
        "multi" => "multi",
        "russian" => "ru",
        "spanish" => "es",
        "turkish" => "tr",
        "vietnamese" => "vi",
        _ => "multi",
    }
}

fn render_inventory(root: &Path, rows: &[InventoryRow]) -> String {
    let mut out = String::new();
    out.push_str("# Upstream Inventory\n\n");
    out.push_str("Generated by `cargo run -p xtask -- inventory-upstreams`.\n\n");
    out.push_str("| Media | Upstream path | Target path | Status | Notes |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for row in rows {
        let upstream = row
            .upstream_path
            .strip_prefix(root.parent().unwrap_or(root))
            .unwrap_or(&row.upstream_path);
        let target = row
            .target_path
            .strip_prefix(root)
            .unwrap_or(&row.target_path);
        out.push_str(&format!(
            "| {} | `{}` | `{}` | {} | {} |\n",
            row.media,
            upstream.display(),
            target.display(),
            row.status,
            row.notes
        ));
    }
    out
}
