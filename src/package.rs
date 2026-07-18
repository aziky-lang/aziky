//! Deterministic, offline-only Aziky package resolution.
//!
//! The resolver intentionally has no network implementation. Every dependency
//! is exact-versioned, checksum-pinned, and read from a caller-selected cache.
//! Lockfiles contain logical package identities only, never host-absolute paths.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const MANIFEST_FILE: &str = "Aziky.toml";
pub const LOCK_FILE: &str = "Aziky.lock";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageOptions {
    pub features: BTreeSet<String>,
    pub default_features: bool,
    pub cache_dir: Option<PathBuf>,
}

impl PackageOptions {
    pub fn defaults_enabled() -> Self {
        Self {
            features: BTreeSet::new(),
            default_features: true,
            cache_dir: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackageId {
    pub name: String,
    pub version: String,
}

impl PackageId {
    pub fn display(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    pub fn namespace(&self) -> String {
        let mut out = String::from("pkg_");
        for byte in self.name.bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
        out.push('_');
        for byte in self.version.bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedPackage {
    pub id: PackageId,
    pub root: PathBuf,
    pub entry: PathBuf,
    pub checksum: String,
    pub features: BTreeSet<String>,
    pub dependencies: BTreeMap<String, PackageId>,
}

#[derive(Clone, Debug)]
pub struct PackageGraph {
    pub manifest_path: PathBuf,
    pub root_dir: PathBuf,
    pub root_entry: PathBuf,
    pub root_id: PackageId,
    pub root_features: BTreeSet<String>,
    pub root_dependencies: BTreeMap<String, PackageId>,
    pub packages: BTreeMap<PackageId, ResolvedPackage>,
    pub lock_text: String,
}

impl PackageGraph {
    pub fn dependency(&self, owner: Option<&PackageId>, alias: &str) -> Option<&ResolvedPackage> {
        let id = match owner {
            Some(owner) => self.packages.get(owner)?.dependencies.get(alias)?,
            None => self.root_dependencies.get(alias)?,
        };
        self.packages.get(id)
    }
}

#[derive(Clone, Debug)]
struct Manifest {
    name: String,
    version: String,
    entry: PathBuf,
    features: BTreeMap<String, Vec<String>>,
    dependencies: BTreeMap<String, Dependency>,
}

#[derive(Clone, Debug)]
struct Dependency {
    package: String,
    version: String,
    checksum: String,
    optional: bool,
    default_features: bool,
    features: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    None,
    Package,
    Features,
    Dependencies,
}

pub fn discover_manifest(input: &Path) -> Result<Option<PathBuf>, String> {
    let start = if input.is_dir() {
        input.to_path_buf()
    } else {
        input
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    };
    let mut directory = fs::canonicalize(&start).map_err(|error| {
        format!(
            "failed to resolve package search root '{}': {error}",
            start.display()
        )
    })?;
    loop {
        let candidate = directory.join(MANIFEST_FILE);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
        if !directory.pop() {
            return Ok(None);
        }
    }
}

pub fn resolve_for_input(
    input: &Path,
    options: &PackageOptions,
) -> Result<Option<PackageGraph>, String> {
    let Some(manifest_path) = discover_manifest(input)? else {
        if !options.features.is_empty() || !options.default_features || options.cache_dir.is_some()
        {
            return Err(format!(
                "package options require an {MANIFEST_FILE} in an ancestor of '{}'",
                input.display()
            ));
        }
        return Ok(None);
    };
    let graph = resolve_manifest(&manifest_path, options)?;
    let lock_path = graph.root_dir.join(LOCK_FILE);
    let actual = fs::read_to_string(&lock_path).map_err(|error| {
        format!(
            "package lockfile '{}' is required and could not be read: {error}; run `aziky package lock {}`",
            lock_path.display(), graph.root_dir.display()
        )
    })?;
    if normalize_newlines(&actual) != graph.lock_text {
        return Err(format!(
            "package lockfile '{}' is stale for the selected manifest, features, or cache contents; run `aziky package lock {}`",
            lock_path.display(),
            graph.root_dir.display()
        ));
    }
    Ok(Some(graph))
}

pub fn write_lock(path: &Path, options: &PackageOptions) -> Result<PackageGraph, String> {
    let manifest_path = if path.is_dir() {
        path.join(MANIFEST_FILE)
    } else if path.file_name().is_some_and(|name| name == MANIFEST_FILE) {
        path.to_path_buf()
    } else {
        discover_manifest(path)?
            .ok_or_else(|| format!("no {MANIFEST_FILE} found for '{}'", path.display()))?
    };
    if !manifest_path.is_file() {
        return Err(format!(
            "package manifest '{}' does not exist",
            manifest_path.display()
        ));
    }
    let graph = resolve_manifest(&manifest_path, options)?;
    let lock_path = graph.root_dir.join(LOCK_FILE);
    fs::write(&lock_path, &graph.lock_text).map_err(|error| {
        format!(
            "failed to write lockfile '{}': {error}",
            lock_path.display()
        )
    })?;
    Ok(graph)
}

fn resolve_manifest(
    manifest_path: &Path,
    options: &PackageOptions,
) -> Result<PackageGraph, String> {
    let manifest_path = fs::canonicalize(manifest_path).map_err(|error| {
        format!(
            "failed to resolve manifest '{}': {error}",
            manifest_path.display()
        )
    })?;
    let root_dir = manifest_path
        .parent()
        .expect("manifest has parent")
        .to_path_buf();
    let root_text = read_utf8(&manifest_path)?;
    let root_manifest = parse_manifest(&root_text, &manifest_path)?;
    let root_id = PackageId {
        name: root_manifest.name.clone(),
        version: root_manifest.version.clone(),
    };
    let cache_dir = match &options.cache_dir {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => root_dir.join(path),
        None => root_dir.join(".aziky").join("cache"),
    };

    let root_features = expand_features(
        &root_manifest,
        &options.features,
        options.default_features,
        &root_id,
    )?;
    let mut manifests = BTreeMap::<PackageId, Manifest>::new();
    let mut roots = BTreeMap::<PackageId, PathBuf>::new();
    let mut checksums = BTreeMap::<PackageId, String>::new();
    let mut requested = BTreeMap::<PackageId, BTreeSet<String>>::new();
    let mut defaults = BTreeMap::<PackageId, bool>::new();
    let mut edges = BTreeMap::<PackageId, BTreeMap<String, PackageId>>::new();
    let mut root_dependencies = BTreeMap::new();
    let mut versions = BTreeMap::<String, String>::new();
    let mut integrity_errors = Vec::new();
    versions.insert(root_id.name.clone(), root_id.version.clone());

    let root_active = active_dependencies(&root_manifest, &root_features);
    let mut queue = VecDeque::new();
    for (alias, dep, forwarded) in root_active {
        let id = dependency_id(&dep);
        ensure_version(&mut versions, &id, &format!("root dependency '{alias}'"))?;
        root_dependencies.insert(alias.clone(), id.clone());
        queue.push_back(Request {
            id,
            expected_checksum: dep.checksum.clone(),
            requested_features: dep.features.union(&forwarded).cloned().collect(),
            default_features: dep.default_features,
            chain: vec![root_id.display()],
        });
    }

    while let Some(request) = queue.pop_front() {
        if request
            .chain
            .iter()
            .any(|entry| entry == &request.id.display())
        {
            let mut cycle = request.chain;
            cycle.push(request.id.display());
            return Err(format!(
                "package dependency cycle detected: {}",
                cycle.join(" -> ")
            ));
        }
        ensure_version(&mut versions, &request.id, "dependency graph")?;
        if let Some(previous) = checksums.get(&request.id) {
            if previous != &request.expected_checksum {
                return Err(format!(
                    "checksum conflict for package '{}': '{}' versus '{}'",
                    request.id.display(),
                    previous,
                    request.expected_checksum
                ));
            }
        }
        let changed = {
            let selected = requested.entry(request.id.clone()).or_default();
            let before = selected.len();
            selected.extend(request.requested_features);
            let default_changed =
                request.default_features && !defaults.get(&request.id).copied().unwrap_or(false);
            if request.default_features {
                defaults.insert(request.id.clone(), true);
            }
            selected.len() != before || default_changed || !manifests.contains_key(&request.id)
        };
        if !changed {
            continue;
        }

        let package_root = cache_dir.join(&request.id.name).join(&request.id.version);
        let actual_checksum = checksum_package(&package_root)?;
        validate_checksum(&request.expected_checksum)?;
        if actual_checksum != request.expected_checksum {
            integrity_errors.push(format!(
                "checksum mismatch for cached package '{}': expected '{}', found '{}' in '{}'",
                request.id.display(),
                request.expected_checksum,
                actual_checksum,
                package_root.display()
            ));
        }
        checksums.insert(request.id.clone(), request.expected_checksum.clone());
        roots.insert(
            request.id.clone(),
            fs::canonicalize(&package_root).map_err(|error| {
                format!(
                    "failed to resolve cached package '{}': {error}",
                    package_root.display()
                )
            })?,
        );
        let dep_manifest_path = package_root.join(MANIFEST_FILE);
        let dep_manifest = parse_manifest(&read_utf8(&dep_manifest_path)?, &dep_manifest_path)?;
        if dep_manifest.name != request.id.name || dep_manifest.version != request.id.version {
            return Err(format!(
                "cached package identity mismatch in '{}': expected '{}', manifest declares '{}@{}'",
                dep_manifest_path.display(),
                request.id.display(),
                dep_manifest.name,
                dep_manifest.version
            ));
        }
        manifests.insert(request.id.clone(), dep_manifest.clone());
        let selected = expand_features(
            &dep_manifest,
            requested.get(&request.id).expect("feature request exists"),
            defaults.get(&request.id).copied().unwrap_or(false),
            &request.id,
        )?;
        let active = active_dependencies(&dep_manifest, &selected);
        let mut package_edges = BTreeMap::new();
        for (alias, dep, forwarded) in active {
            let child = dependency_id(&dep);
            ensure_version(
                &mut versions,
                &child,
                &format!("dependency '{}::{alias}'", request.id.display()),
            )?;
            package_edges.insert(alias, child.clone());
            let mut chain = request.chain.clone();
            chain.push(request.id.display());
            queue.push_back(Request {
                id: child,
                expected_checksum: dep.checksum.clone(),
                requested_features: dep.features.union(&forwarded).cloned().collect(),
                default_features: dep.default_features,
                chain,
            });
        }
        edges.insert(request.id, package_edges);
    }

    if let Some(error) = integrity_errors.into_iter().next() {
        return Err(error);
    }

    let mut packages = BTreeMap::new();
    for (id, manifest) in manifests {
        let root = roots.remove(&id).expect("resolved root");
        let entry = safe_join(&root, &manifest.entry, "package entry")?;
        if !entry.is_file() {
            return Err(format!(
                "package '{}' entry '{}' does not exist",
                id.display(),
                entry.display()
            ));
        }
        let features = expand_features(
            &manifest,
            requested.get(&id).expect("requested features"),
            defaults.get(&id).copied().unwrap_or(false),
            &id,
        )?;
        packages.insert(
            id.clone(),
            ResolvedPackage {
                id: id.clone(),
                root,
                entry: fs::canonicalize(entry).map_err(|error| error.to_string())?,
                checksum: checksums.remove(&id).expect("resolved checksum"),
                features,
                dependencies: edges.remove(&id).unwrap_or_default(),
            },
        );
    }

    let root_entry = safe_join(&root_dir, &root_manifest.entry, "root package entry")?;
    if !root_entry.is_file() {
        return Err(format!(
            "root package entry '{}' does not exist",
            root_entry.display()
        ));
    }
    let manifest_checksum = sha256_tag(root_text.as_bytes());
    let lock_text = render_lock(
        &root_id,
        &manifest_checksum,
        &root_features,
        &root_dependencies,
        &packages,
    );
    Ok(PackageGraph {
        manifest_path,
        root_dir,
        root_entry: fs::canonicalize(root_entry).map_err(|error| error.to_string())?,
        root_id,
        root_features,
        root_dependencies,
        packages,
        lock_text,
    })
}

#[derive(Clone, Debug)]
struct Request {
    id: PackageId,
    expected_checksum: String,
    requested_features: BTreeSet<String>,
    default_features: bool,
    chain: Vec<String>,
}

fn dependency_id(dep: &Dependency) -> PackageId {
    PackageId {
        name: dep.package.clone(),
        version: dep.version.clone(),
    }
}

fn ensure_version(
    versions: &mut BTreeMap<String, String>,
    id: &PackageId,
    context: &str,
) -> Result<(), String> {
    if let Some(previous) = versions.get(&id.name) {
        if previous != &id.version {
            return Err(format!(
                "package version conflict for '{}': versions '{}' and '{}' are both required ({context}); Aziky currently uses deterministic single-version resolution",
                id.name, previous, id.version
            ));
        }
    } else {
        versions.insert(id.name.clone(), id.version.clone());
    }
    Ok(())
}

fn active_dependencies(
    manifest: &Manifest,
    features: &BTreeSet<String>,
) -> Vec<(String, Dependency, BTreeSet<String>)> {
    let mut forwarded = BTreeMap::<String, BTreeSet<String>>::new();
    let mut enabled_optional = BTreeSet::new();
    for feature in features {
        if let Some(entries) = manifest.features.get(feature) {
            for entry in entries {
                if let Some(alias) = entry.strip_prefix("dep:") {
                    enabled_optional.insert(alias.to_string());
                } else if let Some((alias, child_feature)) = entry.split_once('/') {
                    enabled_optional.insert(alias.to_string());
                    forwarded
                        .entry(alias.to_string())
                        .or_default()
                        .insert(child_feature.to_string());
                }
            }
        }
    }
    manifest
        .dependencies
        .iter()
        .filter_map(|(alias, dep)| {
            if dep.optional && !enabled_optional.contains(alias) {
                None
            } else {
                Some((
                    alias.clone(),
                    dep.clone(),
                    forwarded.remove(alias).unwrap_or_default(),
                ))
            }
        })
        .collect()
}

fn expand_features(
    manifest: &Manifest,
    requested: &BTreeSet<String>,
    defaults_enabled: bool,
    id: &PackageId,
) -> Result<BTreeSet<String>, String> {
    let mut selected = requested.clone();
    if defaults_enabled && manifest.features.contains_key("default") {
        selected.insert("default".to_string());
    }
    let mut queue: VecDeque<String> = selected.iter().cloned().collect();
    while let Some(feature) = queue.pop_front() {
        let entries = manifest.features.get(&feature).ok_or_else(|| {
            format!(
                "package '{}' does not define requested feature '{feature}'",
                id.display()
            )
        })?;
        for entry in entries {
            if entry.starts_with("dep:") {
                let alias = &entry[4..];
                match manifest.dependencies.get(alias) {
                    Some(dep) if dep.optional => {}
                    Some(_) => {
                        return Err(format!(
                            "feature '{feature}' in '{}' uses 'dep:{alias}', but dependency '{alias}' is not optional",
                            id.display()
                        ));
                    }
                    None => {
                        return Err(format!(
                            "feature '{feature}' in '{}' refers to unknown dependency '{alias}'",
                            id.display()
                        ));
                    }
                }
            } else if let Some((alias, child_feature)) = entry.split_once('/') {
                if alias.is_empty()
                    || child_feature.is_empty()
                    || !manifest.dependencies.contains_key(alias)
                {
                    return Err(format!(
                        "feature '{feature}' in '{}' has invalid dependency feature '{entry}'",
                        id.display()
                    ));
                }
            } else {
                if !manifest.features.contains_key(entry) {
                    return Err(format!(
                        "feature '{feature}' in '{}' refers to unknown feature '{entry}'",
                        id.display()
                    ));
                }
                if selected.insert(entry.clone()) {
                    queue.push_back(entry.clone());
                }
            }
        }
    }
    Ok(selected)
}

fn parse_manifest(text: &str, path: &Path) -> Result<Manifest, String> {
    let mut section = Section::None;
    let mut package = BTreeMap::<String, String>::new();
    let mut features = BTreeMap::new();
    let mut dependencies = BTreeMap::new();
    for (index, raw) in normalize_newlines(text).lines().enumerate() {
        let line_number = index + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = match line {
                "[package]" => Section::Package,
                "[features]" => Section::Features,
                "[dependencies]" => Section::Dependencies,
                _ => {
                    return manifest_error(
                        path,
                        line_number,
                        format!("unsupported section '{line}'"),
                    );
                }
            };
            continue;
        }
        let (key, value) = split_assignment(line)
            .ok_or_else(|| format!("{}:{line_number}: expected `key = value`", path.display()))?;
        validate_name(key, "manifest key")
            .map_err(|error| format!("{}:{line_number}: {error}", path.display()))?;
        match section {
            Section::Package => {
                let parsed = parse_string(value)
                    .map_err(|error| format!("{}:{line_number}: {error}", path.display()))?;
                if package.insert(key.to_string(), parsed).is_some() {
                    return manifest_error(
                        path,
                        line_number,
                        format!("duplicate package key '{key}'"),
                    );
                }
            }
            Section::Features => {
                let parsed = parse_string_array(value)
                    .map_err(|error| format!("{}:{line_number}: {error}", path.display()))?;
                if features.insert(key.to_string(), parsed).is_some() {
                    return manifest_error(path, line_number, format!("duplicate feature '{key}'"));
                }
            }
            Section::Dependencies => {
                let dep = parse_dependency(key, value)
                    .map_err(|error| format!("{}:{line_number}: {error}", path.display()))?;
                if dependencies.insert(key.to_string(), dep).is_some() {
                    return manifest_error(
                        path,
                        line_number,
                        format!("duplicate dependency alias '{key}'"),
                    );
                }
            }
            Section::None => {
                return manifest_error(path, line_number, "assignment appears before a section");
            }
        }
    }
    for key in package.keys() {
        if !matches!(key.as_str(), "name" | "version" | "entry") {
            return Err(format!(
                "{}: unsupported [package] key '{key}'",
                path.display()
            ));
        }
    }
    let name = package
        .remove("name")
        .ok_or_else(|| format!("{}: [package].name is required", path.display()))?;
    validate_name(&name, "package name").map_err(|error| format!("{}: {error}", path.display()))?;
    let version = package
        .remove("version")
        .ok_or_else(|| format!("{}: [package].version is required", path.display()))?;
    validate_version(&version).map_err(|error| format!("{}: {error}", path.display()))?;
    let entry_text = package
        .remove("entry")
        .unwrap_or_else(|| "src/main.azk".to_string());
    if entry_text.contains('\\') {
        return Err(format!(
            "{}: package entry must use portable '/' separators",
            path.display()
        ));
    }
    let entry = PathBuf::from(entry_text);
    validate_relative_path(&entry, "package entry")
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Manifest {
        name,
        version,
        entry,
        features,
        dependencies,
    })
}

fn parse_dependency(alias: &str, value: &str) -> Result<Dependency, String> {
    validate_module_alias(alias)?;
    if !value.starts_with('{') || !value.ends_with('}') {
        return Err(
            "dependency must be an inline table `{ version = \"...\", checksum = \"sha256:...\" }`"
                .to_string(),
        );
    }
    let fields = split_top_level(&value[1..value.len() - 1], ',')?;
    let mut values = BTreeMap::new();
    for field in fields {
        if field.trim().is_empty() {
            continue;
        }
        let (key, raw) = split_assignment(field.trim())
            .ok_or_else(|| format!("invalid dependency field '{field}'"))?;
        if values
            .insert(key.to_string(), raw.trim().to_string())
            .is_some()
        {
            return Err(format!("duplicate dependency field '{key}'"));
        }
    }
    for key in values.keys() {
        if !matches!(
            key.as_str(),
            "package" | "version" | "checksum" | "optional" | "default-features" | "features"
        ) {
            return Err(format!("unsupported dependency field '{key}'"));
        }
    }
    let package = match values.remove("package") {
        Some(v) => parse_string(&v)?,
        None => alias.to_string(),
    };
    validate_name(&package, "dependency package name")?;
    let version = parse_string(
        &values
            .remove("version")
            .ok_or_else(|| "dependency version is required".to_string())?,
    )?;
    validate_version(&version)?;
    let checksum = parse_string(
        &values
            .remove("checksum")
            .ok_or_else(|| "dependency checksum is required".to_string())?,
    )?;
    validate_checksum(&checksum)?;
    let optional = values
        .remove("optional")
        .map(|v| parse_bool(&v))
        .transpose()?
        .unwrap_or(false);
    let default_features = values
        .remove("default-features")
        .map(|v| parse_bool(&v))
        .transpose()?
        .unwrap_or(true);
    let features = values
        .remove("features")
        .map(|v| parse_string_array(&v))
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .collect();
    Ok(Dependency {
        package,
        version,
        checksum,
        optional,
        default_features,
        features,
    })
}

fn render_lock(
    root: &PackageId,
    manifest_checksum: &str,
    root_features: &BTreeSet<String>,
    root_dependencies: &BTreeMap<String, PackageId>,
    packages: &BTreeMap<PackageId, ResolvedPackage>,
) -> String {
    let mut out = String::new();
    out.push_str("# Generated by Aziky. Do not edit.\nlock-version = 1\n");
    out.push_str(&format!("root = {}\n", quote(&root.display())));
    out.push_str(&format!(
        "manifest-checksum = {}\n",
        quote(manifest_checksum)
    ));
    out.push_str(&format!(
        "root-features = {}\n",
        render_array(root_features.iter().map(String::as_str))
    ));
    out.push_str(&format!(
        "root-dependencies = {}\n",
        render_array(
            root_dependencies
                .iter()
                .map(|(alias, id)| format!("{alias}={}", id.display()))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
        )
    ));
    for package in packages.values() {
        out.push_str("\n[[package]]\n");
        out.push_str(&format!(
            "name = {}\nversion = {}\nchecksum = {}\n",
            quote(&package.id.name),
            quote(&package.id.version),
            quote(&package.checksum)
        ));
        out.push_str(&format!(
            "features = {}\n",
            render_array(package.features.iter().map(String::as_str))
        ));
        let dependencies: Vec<String> = package
            .dependencies
            .iter()
            .map(|(alias, id)| format!("{alias}={}", id.display()))
            .collect();
        out.push_str(&format!(
            "dependencies = {}\n",
            render_array(dependencies.iter().map(String::as_str))
        ));
    }
    out
}

pub fn checksum_package(root: &Path) -> Result<String, String> {
    if !root.is_dir() {
        return Err(format!(
            "offline package cache entry '{}' is missing; Aziky never fetches dependencies implicitly",
            root.display()
        ));
    }
    let mut files = Vec::new();
    collect_package_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if files.is_empty() {
        return Err(format!(
            "cached package '{}' contains no manifest or .azk source files",
            root.display()
        ));
    }
    let mut framed = Vec::new();
    for (relative, path) in files {
        let bytes = fs::read(&path).map_err(|error| {
            format!(
                "failed to read cached package file '{}': {error}",
                path.display()
            )
        })?;
        framed.extend_from_slice(&(relative.len() as u64).to_le_bytes());
        framed.extend_from_slice(relative.as_bytes());
        framed.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        framed.extend_from_slice(&bytes);
    }
    Ok(sha256_tag(&framed))
}

fn collect_package_files(
    root: &Path,
    directory: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .map_err(|error| {
            format!(
                "failed to read package directory '{}': {error}",
                directory.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "failed to enumerate package directory '{}': {error}",
                directory.display()
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let ty = entry.file_type().map_err(|error| error.to_string())?;
        let path = entry.path();
        if ty.is_symlink() {
            return Err(format!(
                "cached package contains unsupported symbolic link '{}'",
                path.display()
            ));
        }
        if ty.is_dir() {
            collect_package_files(root, &path, out)?;
        } else if ty.is_file() {
            let relative_path = path.strip_prefix(root).expect("package child");
            let include = path.file_name().is_some_and(|name| name == MANIFEST_FILE)
                || path.extension().is_some_and(|extension| extension == "azk");
            if include {
                let relative = portable_path(relative_path)?;
                out.push((relative, path));
            }
        } else {
            return Err(format!(
                "cached package contains unsupported file type '{}'",
                path.display()
            ));
        }
    }
    Ok(())
}

fn portable_path(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| format!("package path '{}' is not UTF-8", path.display()))?,
            ),
            _ => return Err(format!("package path '{}' is not portable", path.display())),
        }
    }
    Ok(parts.join("/"))
}

fn safe_join(root: &Path, relative: &Path, label: &str) -> Result<PathBuf, String> {
    validate_relative_path(relative, label)?;
    Ok(root.join(relative))
}

fn validate_relative_path(path: &Path, label: &str) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(format!(
            "{label} must be a non-empty relative portable path"
        ));
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!(
                "{label} '{}' must not contain '.', '..', a root, or a platform prefix",
                path.display()
            ));
        }
    }
    portable_path(path)?;
    Ok(())
}

fn validate_name(name: &str, label: &str) -> Result<(), String> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(format!(
            "{label} '{name}' must start with an ASCII letter or '_' and contain only ASCII letters, digits, '_' or '-'"
        ));
    }
    Ok(())
}

fn validate_module_alias(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(format!(
            "dependency alias '{name}' must be a valid Aziky module identifier"
        ));
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), String> {
    let (without_build, build) = version
        .split_once('+')
        .map_or((version, None), |(left, right)| (left, Some(right)));
    if without_build.contains('+') || build.is_some_and(|value| value.contains('+')) {
        return Err(format!(
            "version '{version}' has more than one build separator"
        ));
    }
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, None), |(left, right)| (left, Some(right)));
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| !valid_numeric_identifier(part)) {
        return Err(format!(
            "version '{version}' must be an exact semantic version such as '1.2.3'"
        ));
    }
    if let Some(prerelease) = prerelease {
        validate_version_identifiers(prerelease, true, version)?;
    }
    if let Some(build) = build {
        validate_version_identifiers(build, false, version)?;
    }
    Ok(())
}

fn valid_numeric_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| ch.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn validate_version_identifiers(
    value: &str,
    reject_numeric_leading_zero: bool,
    full_version: &str,
) -> Result<(), String> {
    for identifier in value.split('.') {
        if identifier.is_empty()
            || !identifier
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
            || (reject_numeric_leading_zero
                && identifier.chars().all(|ch| ch.is_ascii_digit())
                && !valid_numeric_identifier(identifier))
        {
            return Err(format!(
                "version '{full_version}' is not a valid exact semantic version"
            ));
        }
    }
    Ok(())
}

fn validate_checksum(checksum: &str) -> Result<(), String> {
    let Some(hex) = checksum.strip_prefix("sha256:") else {
        return Err(
            "checksum must use `sha256:` followed by 64 lowercase hexadecimal digits".to_string(),
        );
    };
    if hex.len() != 64
        || !hex
            .chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
    {
        return Err(
            "checksum must use `sha256:` followed by 64 lowercase hexadecimal digits".to_string(),
        );
    }
    Ok(())
}

fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
        }
        if ch == '#' && !quoted {
            return &line[..index];
        }
    }
    line
}

fn split_assignment(line: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    for (index, ch) in line.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        }
        if ch == '=' && !quoted {
            return Some((line[..index].trim(), line[index + 1..].trim()));
        }
    }
    None
}

fn split_top_level(input: &str, separator: char) -> Result<Vec<&str>, String> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        match ch {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            _ if ch == separator && depth == 0 => {
                out.push(&input[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
        if depth < 0 {
            return Err("unbalanced inline value".to_string());
        }
    }
    if quoted || depth != 0 {
        return Err("unterminated quoted or inline value".to_string());
    }
    out.push(&input[start..]);
    Ok(out)
}

fn parse_string(value: &str) -> Result<String, String> {
    if !value.starts_with('"') || !value.ends_with('"') || value.len() < 2 {
        return Err(format!("expected quoted string, found '{value}'"));
    }
    let inner = &value[1..value.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some(other) => return Err(format!("unsupported string escape '\\{other}'")),
            None => return Err("unterminated string escape".to_string()),
        }
    }
    Ok(out)
}

fn parse_string_array(value: &str) -> Result<Vec<String>, String> {
    if !value.starts_with('[') || !value.ends_with(']') {
        return Err(format!("expected string array, found '{value}'"));
    }
    let mut out = Vec::new();
    for part in split_top_level(&value[1..value.len() - 1], ',')? {
        if !part.trim().is_empty() {
            out.push(parse_string(part.trim())?);
        }
    }
    Ok(out)
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("expected boolean, found '{value}'")),
    }
}

fn manifest_error<T>(
    path: &Path,
    line: usize,
    message: impl std::fmt::Display,
) -> Result<T, String> {
    Err(format!("{}:{line}: {message}", path.display()))
}

fn read_utf8(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("'{}' is not valid UTF-8", path.display()))
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn render_array<'a>(values: impl Iterator<Item = &'a str>) -> String {
    format!("[{}]", values.map(quote).collect::<Vec<_>>().join(", "))
}

fn sha256_tag(bytes: &[u8]) -> String {
    let digest = sha256(bytes);
    let mut out = String::from("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut message = input.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    let mut h = INITIAL;
    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().expect("four bytes"));
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (state, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *state = state.wrapping_add(value);
        }
    }
    let mut out = [0u8; 32];
    for (chunk, value) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&value.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_standard_vector() {
        assert_eq!(
            sha256_tag(b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn manifest_parses_features_and_exact_dependency() {
        let checksum = format!("sha256:{}", "0".repeat(64));
        let text = format!(
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\nentry = \"main.azk\"\n\n[features]\ndefault = [\"dep:math\"]\n\n[dependencies]\nmath = {{ version = \"2.0.0\", checksum = \"{checksum}\", optional = true }}\n"
        );
        let manifest = parse_manifest(&text, Path::new("Aziky.toml")).expect("valid manifest");
        assert_eq!(manifest.name, "app");
        assert!(manifest.dependencies["math"].optional);
    }

    #[test]
    fn semantic_versions_are_exact_and_canonical() {
        assert!(validate_version("1.2.3-alpha.1+linux-x86_64").is_err());
        assert!(validate_version("1.2.3-alpha.1+linux-x86-64").is_ok());
        assert!(validate_version("1.2").is_err());
        assert!(validate_version("01.2.3").is_err());
        assert!(validate_version("1.2.3-").is_err());
        assert!(validate_version("1.2.3-alpha.01").is_err());
    }
}
