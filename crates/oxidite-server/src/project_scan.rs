//! Bounded, non-blocking project detector for Oxidite's session startup banner.
//!
//! Guarantees:
//! - Runs all filesystem I/O inside `tokio::task::spawn_blocking` (never blocks async workers).
//! - Hard caps on entries (`25_000`), depth (`12`), manifest reads (`64`), and wall-clock budget
//!   (`500ms`) so huge monorepos never stall startup.
//! - Skips symlinks and heavy build/dependency directories (`.git`, `node_modules`, `target`, etc.).
//! - Measures source bytes per language to report languages in descending usage order (most-used
//!   first on the left), and discovers frameworks, libraries, and databases across monorepo subdirs.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

const MAX_SCAN_ENTRIES: usize = 25_000;
const MAX_SCAN_DEPTH: usize = 12;
const MAX_SCAN_BUDGET: Duration = Duration::from_millis(500);
const MAX_MANIFEST_READS: usize = 64;
const MAX_SOURCE_IMPORT_SCANS: usize = 32;
const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_SOURCE_SNIPPET_BYTES: usize = 8 * 1024;

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".venv",
    "venv",
    "env",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".turbo",
    ".cache",
    "vendor",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProjectReport {
    /// Uppercase folder basename used for the stylized header wordmark (e.g. "OXIDITE").
    pub wordmark: String,
    /// Detected root package/project name or folder name.
    pub name: String,
    /// Display path of the scanned root.
    pub path: String,
    pub git_branch: Option<String>,
    /// Comma-separated languages ordered sequentially from most-used on the left.
    pub language: Option<String>,
    /// Languages ordered sequentially from most-used (bytes / file count) to least-used.
    pub languages: Vec<String>,
    /// Frameworks and notable libraries detected across manifests and imports.
    pub frameworks: Vec<String>,
    /// Databases, caches, and data stores detected across drivers, ORMs, and compose/config files.
    pub databases: Vec<String>,
    pub pkg_manager: Option<String>,
    pub prod_deps: Option<usize>,
    pub dev_deps: Option<usize>,
    pub files: usize,
    pub directories: usize,
    pub truncated: bool,
    pub scan_time_ms: u64,
}

#[derive(Default)]
struct LangStats {
    bytes: u64,
    files: usize,
}

#[derive(Default)]
struct DiscoveredFiles {
    cargo_tomls: Vec<PathBuf>,
    go_mods: Vec<PathBuf>,
    package_jsons: Vec<PathBuf>,
    pyproject_tomls: Vec<PathBuf>,
    requirements_txts: Vec<PathBuf>,
    pipfiles: Vec<PathBuf>,
    compose_files: Vec<PathBuf>,
    prisma_schemas: Vec<PathBuf>,
    python_sources: Vec<PathBuf>,
    go_sources: Vec<PathBuf>,
}

/// Runs a bounded, non-blocking project scan on Tokio's blocking thread pool.
pub async fn scan_project_dir(root: PathBuf) -> Result<ProjectReport, String> {
    tokio::task::spawn_blocking(move || scan_project_blocking(&root))
        .await
        .map_err(|e| format!("project scan task failed: {e}"))?
}

pub fn scan_project_blocking(root: &Path) -> Result<ProjectReport, String> {
    let started = Instant::now();
    if !root.is_dir() {
        return Err("Target path is not a directory".to_string());
    }

    let folder_name = root
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("WORKSPACE")
        .to_string();

    let wordmark = folder_name.to_uppercase();
    let mut name = folder_name;

    let git_branch = read_git_branch(root);

    // 1. Bounded single-pass walk for files, directories, language byte usage, and manifests.
    let mut files = 0usize;
    let mut directories = 0usize;
    let mut truncated = false;
    let mut lang_stats: HashMap<&'static str, LangStats> = HashMap::new();
    let mut discovered = DiscoveredFiles::default();
    let mut databases = BTreeSet::new();
    let mut frameworks = BTreeSet::new();
    let mut manifest_languages = BTreeSet::<&'static str>::new();
    let mut pkg_managers = Vec::<String>::new();

    if root.join("supabase/config.toml").is_file() {
        databases.insert("Supabase".to_string());
    }

    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if files + directories >= MAX_SCAN_ENTRIES || started.elapsed() >= MAX_SCAN_BUDGET {
            truncated = true;
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            let entry_path = entry.path();

            if ft.is_dir() {
                if IGNORED_DIRS.contains(&name_str.as_ref()) {
                    continue;
                }
                directories += 1;
                if depth < MAX_SCAN_DEPTH {
                    stack.push((entry_path, depth + 1));
                }
            } else if ft.is_file() {
                files += 1;
                classify_discovered_file(
                    root,
                    &dir,
                    &entry_path,
                    &name_str,
                    &mut discovered,
                    &mut frameworks,
                    &mut databases,
                    &mut manifest_languages,
                );

                // Skip minified bundles so vendor assets don't dominate JS/CSS usage totals.
                let lower = name_str.to_ascii_lowercase();
                if lower.ends_with(".min.js") || lower.ends_with(".min.css") {
                    continue;
                }

                if let Some(ext) = entry_path.extension().and_then(|e| e.to_str())
                    && let Some(lang) = ext_to_language(ext)
                {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(1).max(1);
                    let stat = lang_stats.entry(lang).or_default();
                    stat.bytes = stat.bytes.saturating_add(size);
                    stat.files += 1;
                }
            }
        }
    }

    let mut manifest_reads = 0usize;
    let mut prod_deps: Option<usize> = None;
    let mut dev_deps: Option<usize> = None;

    // Sort manifest lists by path depth (root manifests first) for deterministic precedence.
    sort_by_depth(&mut discovered.cargo_tomls);
    sort_by_depth(&mut discovered.go_mods);
    sort_by_depth(&mut discovered.package_jsons);
    sort_by_depth(&mut discovered.pyproject_tomls);
    sort_by_depth(&mut discovered.requirements_txts);
    sort_by_depth(&mut discovered.pipfiles);
    sort_by_depth(&mut discovered.compose_files);
    sort_by_depth(&mut discovered.prisma_schemas);

    // 2. Inspect Cargo.toml manifests
    for cargo_path in &discovered.cargo_tomls {
        if manifest_reads >= MAX_MANIFEST_READS || started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        manifest_reads += 1;
        let Ok(raw) = read_capped_string(cargo_path, MAX_READ_BYTES) else {
            continue;
        };
        manifest_languages.insert("Rust");
        push_unique_pm(&mut pkg_managers, "cargo");
        if cargo_path.parent() == Some(root)
            && let Some(pkg_name) = parse_toml_package_name(&raw)
        {
            name = pkg_name;
        }
        let (p, d) = count_cargo_deps(&raw, &mut frameworks, &mut databases);
        if p > 0 || d > 0 {
            prod_deps = Some(prod_deps.unwrap_or(0) + p);
            dev_deps = Some(dev_deps.unwrap_or(0) + d);
        }
    }

    // 3. Inspect go.mod manifests (at root or monorepo subdirs like backend/, cmd/, api/)
    for gomod_path in &discovered.go_mods {
        if manifest_reads >= MAX_MANIFEST_READS || started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        manifest_reads += 1;
        let Ok(raw) = read_capped_string(gomod_path, MAX_READ_BYTES) else {
            continue;
        };
        manifest_languages.insert("Go");
        push_unique_pm(&mut pkg_managers, "go");
        let (mod_name, direct_deps) = parse_go_mod(&raw, &mut frameworks, &mut databases);
        if gomod_path.parent() == Some(root)
            && let Some(mn) = mod_name
        {
            // Keep existing folder capitalization when the basename matches case-insensitively.
            if !mn.eq_ignore_ascii_case(&name) {
                name = mn;
            }
        }
        if direct_deps > 0 {
            prod_deps = Some(prod_deps.unwrap_or(0) + direct_deps);
        }
    }

    // 4. Inspect package.json manifests (root or subdirs like web/, frontend/, server/)
    for pkg_path in &discovered.package_jsons {
        if manifest_reads >= MAX_MANIFEST_READS || started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        manifest_reads += 1;
        let Ok(raw) = read_capped_string(pkg_path, MAX_READ_BYTES) else {
            continue;
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let pkg_dir = pkg_path.parent().unwrap_or(root);
        let is_ts = pkg_dir.join("tsconfig.json").is_file() || root.join("tsconfig.json").is_file();
        if is_ts
            || (!lang_stats.contains_key("TypeScript") && !lang_stats.contains_key("JavaScript"))
        {
            manifest_languages.insert(if is_ts { "TypeScript" } else { "JavaScript" });
        }

        // Only a root package.json overrides the top-level repo name; nested web/package.json must not.
        if pkg_dir == root
            && discovered.go_mods.iter().all(|p| p.parent() != Some(root))
            && discovered
                .cargo_tomls
                .iter()
                .all(|p| p.parent() != Some(root))
            && let Some(n) = val.get("name").and_then(|v| v.as_str())
            && !n.is_empty()
        {
            name = n.to_string();
        }

        let js_pm = detect_js_pkg_manager(pkg_dir, root);
        push_unique_pm(&mut pkg_managers, js_pm);
        if js_pm == "bun" || pkg_dir.join("bunfig.toml").is_file() {
            frameworks.insert("Bun".to_string());
        }

        let deps_obj = val.get("dependencies").and_then(|v| v.as_object());
        let dev_obj = val.get("devDependencies").and_then(|v| v.as_object());
        let p_count = deps_obj.map_or(0, serde_json::Map::len);
        let d_count = dev_obj.map_or(0, serde_json::Map::len);
        prod_deps = Some(prod_deps.unwrap_or(0) + p_count);
        dev_deps = Some(dev_deps.unwrap_or(0) + d_count);

        for key in deps_obj
            .into_iter()
            .flat_map(|m| m.keys())
            .chain(dev_obj.into_iter().flat_map(|m| m.keys()))
        {
            detect_js_package(key, &mut frameworks, &mut databases);
        }
    }

    // 5. Inspect Python manifests (`pyproject.toml`, `requirements*.txt`, `Pipfile`)
    for py_path in &discovered.pyproject_tomls {
        if manifest_reads >= MAX_MANIFEST_READS || started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        manifest_reads += 1;
        let Ok(raw) = read_capped_string(py_path, MAX_READ_BYTES) else {
            continue;
        };
        manifest_languages.insert("Python");
        let py_dir = py_path.parent().unwrap_or(root);
        let py_pm = if py_dir.join("uv.lock").is_file() || root.join("uv.lock").is_file() {
            "uv"
        } else if py_dir.join("poetry.lock").is_file() || root.join("poetry.lock").is_file() {
            "poetry"
        } else {
            "pip"
        };
        push_unique_pm(&mut pkg_managers, py_pm);
        let py_deps = parse_pyproject_deps(&raw, &mut frameworks, &mut databases);
        if py_deps > 0 {
            prod_deps = Some(prod_deps.unwrap_or(0) + py_deps);
        }
    }

    for req_path in &discovered.requirements_txts {
        if manifest_reads >= MAX_MANIFEST_READS || started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        manifest_reads += 1;
        let Ok(raw) = read_capped_string(req_path, MAX_READ_BYTES) else {
            continue;
        };
        manifest_languages.insert("Python");
        push_unique_pm(&mut pkg_managers, "pip");
        let req_deps = parse_requirements_txt(&raw, &mut frameworks, &mut databases);
        if req_deps > 0 {
            prod_deps = Some(prod_deps.unwrap_or(0) + req_deps);
        }
    }

    for pipfile_path in &discovered.pipfiles {
        if manifest_reads >= MAX_MANIFEST_READS || started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        manifest_reads += 1;
        let Ok(raw) = read_capped_string(pipfile_path, MAX_READ_BYTES) else {
            continue;
        };
        manifest_languages.insert("Python");
        push_unique_pm(&mut pkg_managers, "pipenv");
        let _ = parse_pyproject_deps(&raw, &mut frameworks, &mut databases);
    }

    // 6. Inspect compose files & Prisma schemas for infrastructure / database services
    for compose_path in &discovered.compose_files {
        if manifest_reads >= MAX_MANIFEST_READS || started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        manifest_reads += 1;
        if let Ok(raw) = read_capped_string(compose_path, MAX_READ_BYTES) {
            detect_compose_databases(&raw, &mut databases);
        }
    }

    for prisma_path in &discovered.prisma_schemas {
        if manifest_reads >= MAX_MANIFEST_READS || started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        manifest_reads += 1;
        frameworks.insert("Prisma".to_string());
        if let Ok(raw) = read_capped_string(prisma_path, MAX_READ_BYTES) {
            detect_prisma_database(&raw, &mut databases);
        }
    }

    // 7. Fallback / supplement: scan top import headers of Python & Go source files so libs like
    //    seaborn/matplotlib or DB drivers show up even when requirements.txt / go.mod omits them.
    for py_src in discovered
        .python_sources
        .iter()
        .take(MAX_SOURCE_IMPORT_SCANS)
    {
        if started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        if let Ok(snippet) = read_capped_string(py_src, MAX_SOURCE_SNIPPET_BYTES) {
            scan_python_imports(&snippet, &mut frameworks, &mut databases);
        }
    }

    for go_src in discovered.go_sources.iter().take(MAX_SOURCE_IMPORT_SCANS) {
        if started.elapsed() >= MAX_SCAN_BUDGET {
            break;
        }
        if let Ok(snippet) = read_capped_string(go_src, MAX_SOURCE_SNIPPET_BYTES) {
            scan_go_imports(&snippet, &mut frameworks, &mut databases);
        }
    }

    // 8. Order every detected language sequentially from most-used (left) to least-used (right).
    let mut ranked_langs: Vec<(&'static str, u64, usize)> = lang_stats
        .into_iter()
        .map(|(lang, st)| (lang, st.bytes, st.files))
        .collect();
    for mlang in &manifest_languages {
        if !ranked_langs.iter().any(|(l, _, _)| l == mlang) {
            ranked_langs.push((*mlang, 0, 0));
        }
    }
    ranked_langs.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.0.cmp(b.0))
    });

    let languages: Vec<String> = ranked_langs
        .into_iter()
        .map(|(lang, _, _)| lang.to_string())
        .collect();
    let language = (!languages.is_empty()).then(|| languages.join(", "));
    let pkg_manager = (!pkg_managers.is_empty()).then(|| pkg_managers.join(", "));
    let scan_time_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    Ok(ProjectReport {
        wordmark,
        name,
        path: root.to_string_lossy().into_owned(),
        git_branch,
        language,
        languages,
        frameworks: frameworks.into_iter().collect(),
        databases: databases.into_iter().collect(),
        pkg_manager,
        prod_deps,
        dev_deps,
        files,
        directories,
        truncated,
        scan_time_ms,
    })
}

#[allow(clippy::too_many_arguments)]
fn classify_discovered_file(
    root: &Path,
    _dir: &Path,
    path: &Path,
    name_str: &str,
    out: &mut DiscoveredFiles,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
    manifest_languages: &mut BTreeSet<&'static str>,
) {
    let lower = name_str.to_ascii_lowercase();
    match lower.as_str() {
        "cargo.toml" => out.cargo_tomls.push(path.to_path_buf()),
        "go.mod" => out.go_mods.push(path.to_path_buf()),
        "package.json" => out.package_jsons.push(path.to_path_buf()),
        "pyproject.toml" => out.pyproject_tomls.push(path.to_path_buf()),
        "pipfile" => out.pipfiles.push(path.to_path_buf()),
        "deno.json" | "deno.jsonc" => {
            frameworks.insert("Deno".to_string());
            manifest_languages.insert("TypeScript");
        }
        "bunfig.toml" => {
            frameworks.insert("Bun".to_string());
        }
        "schema.prisma" => out.prisma_schemas.push(path.to_path_buf()),
        "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml" => {
            out.compose_files.push(path.to_path_buf());
        }
        _ if lower.starts_with("requirements") && lower.ends_with(".txt") => {
            out.requirements_txts.push(path.to_path_buf());
        }
        _ => {}
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        match ext.to_ascii_lowercase().as_str() {
            "py" | "ipynb" if out.python_sources.len() < MAX_SOURCE_IMPORT_SCANS * 2 => {
                out.python_sources.push(path.to_path_buf());
            }
            "go" if out.go_sources.len() < MAX_SOURCE_IMPORT_SCANS * 2 => {
                out.go_sources.push(path.to_path_buf());
            }
            "sqlite" | "sqlite3" => {
                databases.insert("SQLite".to_string());
            }
            "db" if path.parent() == Some(root) => {
                databases.insert("SQLite".to_string());
            }
            _ => {}
        }
    }
}

fn sort_by_depth(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
}

fn push_unique_pm(pms: &mut Vec<String>, candidate: &str) {
    if !pms.iter().any(|existing| existing == candidate) {
        pms.push(candidate.to_string());
    }
}

fn read_capped_string(path: &Path, limit: usize) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; limit];
    let mut total = 0usize;
    while total < limit {
        let n = f.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    buf.truncate(total);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Reads HEAD from `.git/HEAD` or resolves a worktree/submodule `.git` file (`gitdir: ...`).
fn read_git_branch(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let head_path = if dot_git.is_dir() {
        dot_git.join("HEAD")
    } else if dot_git.is_file() {
        let pointer = fs::read_to_string(&dot_git).ok()?;
        let rel = pointer.trim().strip_prefix("gitdir:")?.trim();
        let target = Path::new(rel);
        if target.is_absolute() {
            target.join("HEAD")
        } else {
            root.join(target).join("HEAD")
        }
    } else {
        return None;
    };

    let head = fs::read_to_string(head_path).ok()?;
    let trimmed = head.trim();
    if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
        Some(branch.to_string())
    } else if trimmed.len() >= 7 {
        Some(trimmed[..7].to_string())
    } else {
        None
    }
}

fn detect_js_pkg_manager(dir: &Path, root: &Path) -> &'static str {
    for d in [dir, root] {
        if d.join("bun.lock").is_file() || d.join("bun.lockb").is_file() {
            return "bun";
        }
        if d.join("pnpm-lock.yaml").is_file() {
            return "pnpm";
        }
        if d.join("yarn.lock").is_file() {
            return "yarn";
        }
    }
    "npm"
}

fn parse_toml_package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for raw in toml.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == "name"
        {
            let val = v.trim().trim_matches(|c| c == '"' || c == '\'');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn count_cargo_deps(
    toml: &str,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
) -> (usize, usize) {
    #[derive(PartialEq)]
    enum Section {
        Other,
        Deps,
        DevDeps,
    }
    let mut sec = Section::Other;
    let mut prod = 0usize;
    let mut dev = 0usize;

    for raw in toml.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            sec = if line == "[dependencies]" || line == "[workspace.dependencies]" {
                Section::Deps
            } else if line == "[dev-dependencies]" {
                Section::DevDeps
            } else {
                Section::Other
            };
            continue;
        }
        if let Some((key, rhs)) = line.split_once('=') {
            let k = key.trim().trim_matches(|c| c == '"' || c == '\'');
            if sec == Section::Deps {
                prod += 1;
            } else if sec == Section::DevDeps {
                dev += 1;
            }
            if sec == Section::Other {
                continue;
            }
            match k {
                "axum" => {
                    frameworks.insert("Axum".into());
                }
                "tokio" => {
                    frameworks.insert("Tokio".into());
                }
                "actix-web" => {
                    frameworks.insert("Actix Web".into());
                }
                "rocket" => {
                    frameworks.insert("Rocket".into());
                }
                "poem" => {
                    frameworks.insert("Poem".into());
                }
                "tonic" => {
                    frameworks.insert("Tonic".into());
                }
                "ratatui" => {
                    frameworks.insert("Ratatui".into());
                }
                "winit" => {
                    frameworks.insert("winit".into());
                }
                "wgpu" => {
                    frameworks.insert("wgpu".into());
                }
                "serde" => {
                    frameworks.insert("Serde".into());
                }
                "tracing" => {
                    frameworks.insert("Tracing".into());
                }
                "reqwest" => {
                    frameworks.insert("Reqwest".into());
                }
                "clap" => {
                    frameworks.insert("Clap".into());
                }
                "russh" => {
                    frameworks.insert("russh".into());
                }
                "rmp-serde" => {
                    frameworks.insert("MessagePack".into());
                }
                "sqlx" => {
                    frameworks.insert("SQLx".into());
                    let rhs_lower = rhs.to_ascii_lowercase();
                    if rhs_lower.contains("postgres") {
                        databases.insert("PostgreSQL".into());
                    }
                    if rhs_lower.contains("mysql") {
                        databases.insert("MySQL".into());
                    }
                    if rhs_lower.contains("sqlite") {
                        databases.insert("SQLite".into());
                    }
                }
                "diesel" => {
                    frameworks.insert("Diesel".into());
                    let rhs_lower = rhs.to_ascii_lowercase();
                    if rhs_lower.contains("postgres") {
                        databases.insert("PostgreSQL".into());
                    }
                    if rhs_lower.contains("mysql") {
                        databases.insert("MySQL".into());
                    }
                    if rhs_lower.contains("sqlite") {
                        databases.insert("SQLite".into());
                    }
                }
                "sea-orm" => {
                    frameworks.insert("SeaORM".into());
                    let rhs_lower = rhs.to_ascii_lowercase();
                    if rhs_lower.contains("postgres") {
                        databases.insert("PostgreSQL".into());
                    }
                    if rhs_lower.contains("mysql") {
                        databases.insert("MySQL".into());
                    }
                    if rhs_lower.contains("sqlite") {
                        databases.insert("SQLite".into());
                    }
                }
                "rusqlite" | "libsql" => {
                    databases.insert("SQLite".into());
                }
                "postgres" | "tokio-postgres" | "deadpool-postgres" => {
                    databases.insert("PostgreSQL".into());
                }
                "mysql" | "mysql_async" => {
                    databases.insert("MySQL".into());
                }
                "mongodb" => {
                    databases.insert("MongoDB".into());
                }
                "redis" | "deadpool-redis" | "fred" => {
                    databases.insert("Redis".into());
                }
                "duckdb" => {
                    databases.insert("DuckDB".into());
                }
                _ => {}
            }
        }
    }
    (prod, dev)
}

fn parse_go_mod(
    raw: &str,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
) -> (Option<String>, usize) {
    let mut mod_name: Option<String> = None;
    let mut direct_deps = 0usize;
    let mut in_require_block = false;

    for line_raw in raw.lines() {
        let trimmed = line_raw.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("module ") {
            let path = rest.split_whitespace().next().unwrap_or("").trim();
            if let Some(last) = path.rsplit('/').next()
                && !last.is_empty()
            {
                mod_name = Some(last.to_string());
            }
            continue;
        }
        if trimmed.starts_with("require (") || trimmed == "require(" {
            in_require_block = true;
            continue;
        }
        if in_require_block {
            if trimmed == ")" {
                in_require_block = false;
                continue;
            }
            let is_indirect = trimmed.contains("// indirect");
            if let Some(pkg) = trimmed.split_whitespace().next() {
                if !is_indirect {
                    direct_deps += 1;
                }
                detect_go_module(pkg, is_indirect, frameworks, databases);
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("require ") {
            let is_indirect = trimmed.contains("// indirect");
            if let Some(pkg) = rest.split_whitespace().next() {
                if !is_indirect {
                    direct_deps += 1;
                }
                detect_go_module(pkg, is_indirect, frameworks, databases);
            }
        }
    }

    (mod_name, direct_deps)
}

fn detect_go_module(
    pkg_raw: &str,
    is_indirect: bool,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
) {
    let pkg = pkg_raw.trim_matches('"').to_ascii_lowercase();

    // Check database drivers even if indirect in multi-package Go modules (e.g. GORM driver subdeps),
    // but only record frameworks for direct dependencies so transitive SDK deps don't clutter.
    let mut matched_db = true;
    if pkg.contains("jackc/pgx")
        || pkg.starts_with("github.com/lib/pq")
        || pkg.contains("gorm.io/driver/postgres")
        || pkg.contains("uptrace/bun/driver/pgdriver")
    {
        databases.insert("PostgreSQL".into());
    } else if pkg.contains("go-sql-driver/mysql") || pkg.contains("gorm.io/driver/mysql") {
        databases.insert("MySQL".into());
    } else if pkg.contains("go.mongodb.org/mongo-driver") {
        databases.insert("MongoDB".into());
    } else if pkg.contains("redis/go-redis") || pkg.contains("go-redis/redis") {
        databases.insert("Redis".into());
    } else if pkg.contains("mattn/go-sqlite3")
        || pkg.contains("modernc.org/sqlite")
        || pkg.contains("gorm.io/driver/sqlite")
    {
        databases.insert("SQLite".into());
    } else if pkg.contains("supabase") {
        databases.insert("Supabase".into());
    } else if pkg.contains("clickhouse/clickhouse-go") {
        databases.insert("ClickHouse".into());
    } else if pkg.contains("go-duckdb") {
        databases.insert("DuckDB".into());
    } else {
        matched_db = false;
    }

    if is_indirect && matched_db {
        return;
    }
    if is_indirect {
        return;
    }

    if pkg.starts_with("github.com/gin-gonic/gin") {
        frameworks.insert("Gin".into());
    } else if pkg.starts_with("github.com/gofiber/fiber") {
        frameworks.insert("Fiber".into());
    } else if pkg.starts_with("github.com/labstack/echo") {
        frameworks.insert("Echo".into());
    } else if pkg.starts_with("github.com/go-chi/chi") {
        frameworks.insert("Chi".into());
    } else if pkg.starts_with("github.com/gorilla/mux") {
        frameworks.insert("Gorilla Mux".into());
    } else if pkg.starts_with("github.com/gorilla/websocket") {
        frameworks.insert("Gorilla WebSocket".into());
    } else if pkg.starts_with("google.golang.org/grpc") {
        frameworks.insert("gRPC".into());
    } else if pkg.starts_with("connectrpc.com/connect") {
        frameworks.insert("ConnectRPC".into());
    } else if pkg.starts_with("gorm.io/gorm") {
        frameworks.insert("GORM".into());
    } else if pkg.starts_with("entgo.io/ent") {
        frameworks.insert("Ent".into());
    } else if pkg.starts_with("github.com/jmoiron/sqlx") {
        frameworks.insert("sqlx".into());
    } else if pkg.starts_with("github.com/spf13/cobra") {
        frameworks.insert("Cobra".into());
    } else if pkg.starts_with("github.com/spf13/viper") {
        frameworks.insert("Viper".into());
    } else if pkg.starts_with("go.uber.org/zap") {
        frameworks.insert("Zap".into());
    } else if pkg.starts_with("github.com/rs/zerolog") {
        frameworks.insert("Zerolog".into());
    } else if pkg.starts_with("github.com/prometheus/client_golang") {
        frameworks.insert("Prometheus".into());
    } else if pkg.starts_with("github.com/nats-io/nats.go") {
        frameworks.insert("NATS".into());
    } else if pkg.starts_with("github.com/segmentio/kafka-go")
        || pkg.starts_with("github.com/ibm/sarama")
        || pkg.starts_with("github.com/confluentinc/confluent-kafka-go")
    {
        frameworks.insert("Kafka".into());
    } else if pkg.starts_with("github.com/charmbracelet/bubbletea") {
        frameworks.insert("Bubble Tea".into());
    } else if pkg.starts_with("github.com/99designs/gqlgen") {
        frameworks.insert("gqlgen".into());
    } else if pkg.starts_with("github.com/stretchr/testify") {
        frameworks.insert("Testify".into());
    }
}

fn scan_go_imports(src: &str, frameworks: &mut BTreeSet<String>, databases: &mut BTreeSet<String>) {
    for line in src.lines() {
        let t = line.trim();
        if let Some(start) = t.find('"')
            && let Some(end) = t[start + 1..].find('"')
        {
            let pkg = &t[start + 1..start + 1 + end];
            if pkg.contains('.') || pkg.contains('/') {
                detect_go_module(pkg, false, frameworks, databases);
            }
        }
    }
}

fn detect_js_package(
    pkg_raw: &str,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
) {
    let pkg = pkg_raw.to_ascii_lowercase();
    match pkg.as_str() {
        "react" | "react-dom" => {
            frameworks.insert("React".into());
        }
        "next" => {
            frameworks.insert("Next.js".into());
        }
        "vite" => {
            frameworks.insert("Vite".into());
        }
        "express" => {
            frameworks.insert("Express".into());
        }
        "fastify" => {
            frameworks.insert("Fastify".into());
        }
        "hono" => {
            frameworks.insert("Hono".into());
        }
        "koa" => {
            frameworks.insert("Koa".into());
        }
        "elysia" => {
            frameworks.insert("Elysia".into());
        }
        "@nestjs/core" => {
            frameworks.insert("NestJS".into());
        }
        "vue" => {
            frameworks.insert("Vue".into());
        }
        "nuxt" => {
            frameworks.insert("Nuxt".into());
        }
        "svelte" => {
            frameworks.insert("Svelte".into());
        }
        "@sveltejs/kit" => {
            frameworks.insert("SvelteKit".into());
        }
        "solid-js" => {
            frameworks.insert("SolidJS".into());
        }
        "@angular/core" => {
            frameworks.insert("Angular".into());
        }
        "astro" => {
            frameworks.insert("Astro".into());
        }
        "@remix-run/react" => {
            frameworks.insert("Remix".into());
        }
        "tailwindcss" => {
            frameworks.insert("Tailwind CSS".into());
        }
        "@tanstack/react-query" | "@tanstack/vue-query" => {
            frameworks.insert("TanStack Query".into());
        }
        "@trpc/server" | "@trpc/client" => {
            frameworks.insert("tRPC".into());
        }
        "graphql" | "@apollo/server" | "@apollo/client" => {
            frameworks.insert("GraphQL".into());
        }
        "socket.io" | "socket.io-client" => {
            frameworks.insert("Socket.IO".into());
        }
        "axios" => {
            frameworks.insert("Axios".into());
        }
        "zustand" => {
            frameworks.insert("Zustand".into());
        }
        "redux" | "@reduxjs/toolkit" => {
            frameworks.insert("Redux".into());
        }
        "zod" => {
            frameworks.insert("Zod".into());
        }
        "three" => {
            frameworks.insert("Three.js".into());
        }
        "recharts" => {
            frameworks.insert("Recharts".into());
        }
        "chart.js" => {
            frameworks.insert("Chart.js".into());
        }
        "d3" => {
            frameworks.insert("D3.js".into());
        }
        "echarts" => {
            frameworks.insert("ECharts".into());
        }
        "@xterm/xterm" | "xterm" => {
            frameworks.insert("xterm.js".into());
        }
        "electron" => {
            frameworks.insert("Electron".into());
        }
        "@tauri-apps/api" => {
            frameworks.insert("Tauri".into());
        }
        "@playwright/test" | "playwright" => {
            frameworks.insert("Playwright".into());
        }
        "vitest" => {
            frameworks.insert("Vitest".into());
        }
        "jest" => {
            frameworks.insert("Jest".into());
        }
        "@types/bun" | "bun-types" | "bun" => {
            frameworks.insert("Bun".into());
        }
        "prisma" | "@prisma/client" => {
            frameworks.insert("Prisma".into());
        }
        "drizzle-orm" => {
            frameworks.insert("Drizzle ORM".into());
        }
        "typeorm" => {
            frameworks.insert("TypeORM".into());
        }
        "sequelize" => {
            frameworks.insert("Sequelize".into());
        }
        "pg" | "postgres" | "@neondatabase/serverless" | "@vercel/postgres" => {
            databases.insert("PostgreSQL".into());
        }
        "mysql" | "mysql2" => {
            databases.insert("MySQL".into());
        }
        "@supabase/supabase-js" | "@supabase/ssr" | "supabase" => {
            databases.insert("Supabase".into());
        }
        "mongodb" => {
            databases.insert("MongoDB".into());
        }
        "mongoose" => {
            frameworks.insert("Mongoose".into());
            databases.insert("MongoDB".into());
        }
        "sqlite3" | "better-sqlite3" | "@libsql/client" => {
            databases.insert("SQLite".into());
        }
        "redis" | "ioredis" | "@upstash/redis" => {
            databases.insert("Redis".into());
        }
        "firebase" | "firebase-admin" => {
            databases.insert("Firebase".into());
        }
        "duckdb" => {
            databases.insert("DuckDB".into());
        }
        "@clickhouse/client" => {
            databases.insert("ClickHouse".into());
        }
        "@elastic/elasticsearch" => {
            databases.insert("Elasticsearch".into());
        }
        _ => {}
    }
}

fn parse_requirements_txt(
    raw: &str,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
) -> usize {
    let mut count = 0usize;
    for line in raw.lines() {
        let stripped = line.split('#').next().unwrap_or("").trim();
        if stripped.is_empty() || stripped.starts_with('-') {
            continue;
        }
        let pkg = stripped
            .split(['=', '<', '>', '~', '!', ';', '[', ' '])
            .next()
            .unwrap_or("")
            .trim();
        if !pkg.is_empty() {
            count += 1;
            detect_python_package(pkg, frameworks, databases);
        }
    }
    count
}

fn parse_pyproject_deps(
    raw: &str,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
) -> usize {
    let mut deps = BTreeSet::new();
    let mut in_dep_array = false;
    let mut in_table_deps = false;

    for line_raw in raw.lines() {
        let line = line_raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_table_deps = line == "[tool.poetry.dependencies]"
                || line == "[tool.poetry.dev-dependencies]"
                || line == "[packages]"
                || line == "[dev-packages]";
            in_dep_array = false;
            continue;
        }
        if in_table_deps {
            if let Some((k, _)) = line.split_once('=') {
                let pkg = k.trim().trim_matches(|c| c == '"' || c == '\'');
                if !pkg.is_empty() && pkg != "python" {
                    deps.insert(pkg.to_string());
                    detect_python_package(pkg, frameworks, databases);
                }
            }
            continue;
        }
        if (line.starts_with("dependencies") || line.starts_with("optional-dependencies"))
            && line.contains('[')
        {
            in_dep_array = !line.contains(']');
            extract_quoted_py_deps(line, &mut deps, frameworks, databases);
            continue;
        }
        if in_dep_array {
            if line.contains(']') {
                in_dep_array = false;
            }
            extract_quoted_py_deps(line, &mut deps, frameworks, databases);
        }
    }

    deps.len()
}

fn extract_quoted_py_deps(
    line: &str,
    seen: &mut BTreeSet<String>,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
) {
    let mut rest = line;
    while let Some(q1) = rest.find(['"', '\'']) {
        let quote = rest.as_bytes()[q1] as char;
        rest = &rest[q1 + 1..];
        let Some(q2) = rest.find(quote) else {
            break;
        };
        let spec = &rest[..q2];
        rest = &rest[q2 + 1..];
        let pkg = spec
            .split(['=', '<', '>', '~', '!', ';', '[', ' '])
            .next()
            .unwrap_or("")
            .trim();
        if !pkg.is_empty() && pkg != "dependencies" {
            seen.insert(pkg.to_string());
            detect_python_package(pkg, frameworks, databases);
        }
    }
}

fn scan_python_imports(
    src: &str,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
) {
    for line in src.lines() {
        let mut t = line.trim().trim_start_matches('"').trim();
        if let Some(stripped) = t.strip_suffix("\\n\",") {
            t = stripped;
        } else if let Some(stripped) = t.strip_suffix("\",") {
            t = stripped;
        }
        if let Some(rest) = t.strip_prefix("import ") {
            for item in rest.split(',') {
                let mod_name = item
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .split('.')
                    .next()
                    .unwrap_or("");
                if !mod_name.is_empty() {
                    detect_python_package(mod_name, frameworks, databases);
                }
            }
        } else if let Some(rest) = t.strip_prefix("from ") {
            let mod_name = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .split('.')
                .next()
                .unwrap_or("");
            if !mod_name.is_empty() {
                detect_python_package(mod_name, frameworks, databases);
            }
        }
    }
}

fn detect_python_package(
    pkg_raw: &str,
    frameworks: &mut BTreeSet<String>,
    databases: &mut BTreeSet<String>,
) {
    let norm = pkg_raw.trim().to_ascii_lowercase().replace('_', "-");
    match norm.as_str() {
        "seaborn" => {
            frameworks.insert("Seaborn".into());
        }
        "matplotlib" => {
            frameworks.insert("Matplotlib".into());
        }
        "pandas" => {
            frameworks.insert("Pandas".into());
        }
        "numpy" => {
            frameworks.insert("NumPy".into());
        }
        "scipy" => {
            frameworks.insert("SciPy".into());
        }
        "scikit-learn" | "sklearn" => {
            frameworks.insert("scikit-learn".into());
        }
        "torch" | "pytorch" => {
            frameworks.insert("PyTorch".into());
        }
        "tensorflow" => {
            frameworks.insert("TensorFlow".into());
        }
        "keras" => {
            frameworks.insert("Keras".into());
        }
        "xgboost" => {
            frameworks.insert("XGBoost".into());
        }
        "lightgbm" => {
            frameworks.insert("LightGBM".into());
        }
        "polars" => {
            frameworks.insert("Polars".into());
        }
        "plotly" => {
            frameworks.insert("Plotly".into());
        }
        "altair" => {
            frameworks.insert("Altair".into());
        }
        "bokeh" => {
            frameworks.insert("Bokeh".into());
        }
        "statsmodels" => {
            frameworks.insert("statsmodels".into());
        }
        "opencv-python" | "cv2" => {
            frameworks.insert("OpenCV".into());
        }
        "transformers" => {
            frameworks.insert("Transformers".into());
        }
        "langchain" => {
            frameworks.insert("LangChain".into());
        }
        "streamlit" => {
            frameworks.insert("Streamlit".into());
        }
        "gradio" => {
            frameworks.insert("Gradio".into());
        }
        "jupyter" | "notebook" | "jupyterlab" => {
            frameworks.insert("Jupyter".into());
        }
        "fastapi" => {
            frameworks.insert("FastAPI".into());
        }
        "django" => {
            frameworks.insert("Django".into());
        }
        "flask" => {
            frameworks.insert("Flask".into());
        }
        "starlette" => {
            frameworks.insert("Starlette".into());
        }
        "tornado" => {
            frameworks.insert("Tornado".into());
        }
        "aiohttp" => {
            frameworks.insert("aiohttp".into());
        }
        "pydantic" => {
            frameworks.insert("Pydantic".into());
        }
        "sqlalchemy" => {
            frameworks.insert("SQLAlchemy".into());
        }
        "sqlmodel" => {
            frameworks.insert("SQLModel".into());
        }
        "celery" => {
            frameworks.insert("Celery".into());
        }
        "pytest" => {
            frameworks.insert("pytest".into());
        }
        "psycopg2" | "psycopg2-binary" | "psycopg" | "asyncpg" => {
            databases.insert("PostgreSQL".into());
        }
        "pymysql" | "mysqlclient" | "mysql-connector-python" | "aiomysql" => {
            databases.insert("MySQL".into());
        }
        "pymongo" | "motor" | "beanie" => {
            databases.insert("MongoDB".into());
        }
        "supabase" => {
            databases.insert("Supabase".into());
        }
        "redis" | "aioredis" => {
            databases.insert("Redis".into());
        }
        "duckdb" => {
            databases.insert("DuckDB".into());
        }
        "sqlite3" | "aiosqlite" => {
            databases.insert("SQLite".into());
        }
        "elasticsearch" => {
            databases.insert("Elasticsearch".into());
        }
        _ => {}
    }
}

fn detect_compose_databases(raw: &str, databases: &mut BTreeSet<String>) {
    for line_raw in raw.lines() {
        let line = line_raw
            .split('#')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !line.starts_with("image:") {
            continue;
        }
        if line.contains("postgres") || line.contains("postgis") || line.contains("timescaledb") {
            databases.insert("PostgreSQL".into());
        }
        if line.contains("mysql") || line.contains("mariadb") {
            databases.insert("MySQL".into());
        }
        if line.contains("mongo") {
            databases.insert("MongoDB".into());
        }
        if line.contains("redis") || line.contains("valkey") {
            databases.insert("Redis".into());
        }
        if line.contains("supabase") {
            databases.insert("Supabase".into());
        }
        if line.contains("clickhouse") {
            databases.insert("ClickHouse".into());
        }
        if line.contains("elasticsearch") || line.contains("opensearch") {
            databases.insert("Elasticsearch".into());
        }
    }
}

fn detect_prisma_database(raw: &str, databases: &mut BTreeSet<String>) {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("provider = \"postgresql\"") || lower.contains("provider = \"postgres\"") {
        databases.insert("PostgreSQL".into());
    }
    if lower.contains("provider = \"mysql\"") {
        databases.insert("MySQL".into());
    }
    if lower.contains("provider = \"sqlite\"") {
        databases.insert("SQLite".into());
    }
    if lower.contains("provider = \"mongodb\"") {
        databases.insert("MongoDB".into());
    }
}

fn ext_to_language(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "go" => Some("Go"),
        "rs" => Some("Rust"),
        "ts" | "tsx" | "mts" | "cts" => Some("TypeScript"),
        "js" | "jsx" | "mjs" | "cjs" => Some("JavaScript"),
        "py" | "pyw" | "pyi" | "ipynb" => Some("Python"),
        "sql" => Some("SQL"),
        "java" => Some("Java"),
        "kt" | "kts" => Some("Kotlin"),
        "cs" => Some("C#"),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Some("C++"),
        "c" | "h" => Some("C"),
        "rb" => Some("Ruby"),
        "php" => Some("PHP"),
        "swift" => Some("Swift"),
        "dart" => Some("Dart"),
        "ex" | "exs" => Some("Elixir"),
        "scala" => Some("Scala"),
        "html" | "htm" => Some("HTML"),
        "css" | "scss" | "sass" | "less" => Some("CSS"),
        "svelte" => Some("Svelte"),
        "vue" => Some("Vue"),
        "sh" | "bash" | "zsh" => Some("Shell"),
        "ps1" | "psm1" => Some("PowerShell"),
        "proto" => Some("Protobuf"),
        "v" | "sv" => Some("Verilog"),
        "tf" => Some("HCL"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_project(dir_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir()
            .join(format!("oxidite_scan_test_{nanos}"))
            .join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scans_rust_and_vite_monorepo_ignoring_heavy_dirs() {
        let root = make_temp_project("Oxidite");
        fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/server"]

[workspace.dependencies]
tokio = "1"
axum = "0.7"
sqlx = "0.8"
"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/server/src")).unwrap();
        fs::write(
            root.join("crates/server/src/main.rs"),
            "fn main() { println!(\"rust primary language by byte count\"); }\n",
        )
        .unwrap();

        fs::create_dir_all(root.join("web/src")).unwrap();
        fs::write(
            root.join("web/package.json"),
            r#"{"name":"oxidite-web","dependencies":{"vite":"^5.0.0"},"devDependencies":{"tailwindcss":"^3.4.0"}}"#,
        )
        .unwrap();
        fs::write(root.join("web/bun.lock"), "").unwrap();
        fs::write(root.join("web/src/main.ts"), "console.log('ok');").unwrap();

        // Ignored directories must not inflate counts
        fs::create_dir_all(root.join("node_modules/fake")).unwrap();
        fs::write(root.join("node_modules/fake/index.js"), "").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/bin.exe"), "").unwrap();

        // Standard git HEAD
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let report = scan_project_blocking(&root).unwrap();
        assert_eq!(report.wordmark, "OXIDITE");
        assert_eq!(report.name, "Oxidite");
        assert_eq!(report.git_branch.as_deref(), Some("main"));
        assert_eq!(report.languages, vec!["Rust", "TypeScript"]);
        assert_eq!(report.language.as_deref(), Some("Rust, TypeScript"));
        assert_eq!(report.pkg_manager.as_deref(), Some("cargo, bun"));
        assert!(report.frameworks.contains(&"Axum".to_string()));
        assert!(report.frameworks.contains(&"Tokio".to_string()));
        assert!(report.frameworks.contains(&"Bun".to_string()));
        assert!(report.frameworks.contains(&"Vite".to_string()));
        assert!(report.frameworks.contains(&"Tailwind CSS".to_string()));
        assert_eq!(report.prod_deps, Some(4)); // 3 workspace + 1 web
        assert_eq!(report.dev_deps, Some(1));
        assert_eq!(report.files, 5);
        assert_eq!(report.directories, 5); // crates, crates/server, crates/server/src, web, web/src

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn scans_go_primary_monorepo_with_react_and_databases_ordered_by_usage() {
        let root = make_temp_project("OrderFlow");
        fs::create_dir_all(root.join("backend/cmd/server")).unwrap();
        fs::write(
            root.join("backend/go.mod"),
            r#"
module github.com/acme/orderflow

go 1.23

require (
    github.com/gin-gonic/gin v1.10.0
    github.com/jackc/pgx/v5 v5.6.0
    github.com/redis/go-redis/v9 v9.5.1
    gorm.io/gorm v1.25.10
)
"#,
        )
        .unwrap();
        fs::write(
            root.join("backend/cmd/server/main.go"),
            "package main\nfunc handler() {}\n".repeat(20),
        )
        .unwrap();

        fs::create_dir_all(root.join("web/src")).unwrap();
        fs::write(
            root.join("web/package.json"),
            r#"{"name":"orderflow-web","dependencies":{"react":"19.0.0","@supabase/supabase-js":"2.0.0"},"devDependencies":{"vite":"6.0.0","tailwindcss":"4.0.0"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("web/src/App.tsx"),
            "export const App = () => null;\n",
        )
        .unwrap();
        fs::write(root.join("web/src/app.css"), "body{}").unwrap();

        let report = scan_project_blocking(&root).unwrap();
        assert_eq!(report.name, "OrderFlow"); // nested web/package.json must not override repo name
        assert_eq!(report.languages, vec!["Go", "TypeScript", "CSS"]);
        assert_eq!(report.language.as_deref(), Some("Go, TypeScript, CSS"));
        assert!(report.frameworks.contains(&"Gin".to_string()));
        assert!(report.frameworks.contains(&"GORM".to_string()));
        assert!(report.frameworks.contains(&"React".to_string()));
        assert!(report.frameworks.contains(&"Vite".to_string()));
        assert!(report.databases.contains(&"PostgreSQL".to_string()));
        assert!(report.databases.contains(&"Redis".to_string()));
        assert!(report.databases.contains(&"Supabase".to_string()));

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn scans_python_data_science_libs_express_and_mongodb() {
        let root = make_temp_project("ml-api");
        fs::write(
            root.join("requirements.txt"),
            "fastapi==0.115.0\nseaborn>=0.13\nmatplotlib\npandas\npsycopg2-binary\n",
        )
        .unwrap();
        fs::write(
            root.join("train.py"),
            "import seaborn as sns\nimport matplotlib.pyplot as plt\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("gateway")).unwrap();
        fs::write(
            root.join("gateway/package.json"),
            r#"{"name":"gateway","dependencies":{"express":"5.0.0","mongoose":"8.0.0"}}"#,
        )
        .unwrap();
        fs::write(root.join("gateway/index.js"), "console.log(1);").unwrap();

        let report = scan_project_blocking(&root).unwrap();
        assert_eq!(report.languages, vec!["Python", "JavaScript"]);
        assert!(report.frameworks.contains(&"Seaborn".to_string()));
        assert!(report.frameworks.contains(&"Matplotlib".to_string()));
        assert!(report.frameworks.contains(&"Pandas".to_string()));
        assert!(report.frameworks.contains(&"FastAPI".to_string()));
        assert!(report.frameworks.contains(&"Express".into()));
        assert!(report.frameworks.contains(&"Mongoose".into()));
        assert!(report.databases.contains(&"PostgreSQL".into()));
        assert!(report.databases.contains(&"MongoDB".into()));

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn reads_git_worktree_pointer_file() {
        let root = make_temp_project("worktree-app");
        let git_meta = root.parent().unwrap().join("actual-gitdir");
        fs::create_dir_all(&git_meta).unwrap();
        fs::write(git_meta.join("HEAD"), "ref: refs/heads/feat/mobile-ux\n").unwrap();
        fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", git_meta.to_string_lossy()),
        )
        .unwrap();

        let report = scan_project_blocking(&root).unwrap();
        assert_eq!(report.wordmark, "WORKTREE-APP");
        assert_eq!(report.git_branch.as_deref(), Some("feat/mobile-ux"));

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }
}
