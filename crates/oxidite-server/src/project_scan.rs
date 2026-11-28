//! Bounded, non-blocking project detector for Oxidite's session startup banner.
//!
//! Guarantees:
//! - Runs all filesystem I/O inside `tokio::task::spawn_blocking` (never blocks async workers).
//! - Hard caps on entries (`25_000`), depth (`12`), and wall-clock budget (`500ms`) so huge
//!   monorepos never stall startup.
//! - Skips symlinks and heavy build/dependency directories (`.git`, `node_modules`, `target`, etc.).
//! - Supports both standard `.git/HEAD` directories and Git worktrees/submodules (`.git` pointer files).

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

const MAX_SCAN_ENTRIES: usize = 25_000;
const MAX_SCAN_DEPTH: usize = 12;
const MAX_SCAN_BUDGET: Duration = Duration::from_millis(500);

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    "__pycache__",
    ".turbo",
    ".cache",
    "vendor",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProjectReport {
    /// Uppercase folder basename used for the stylized header wordmark (e.g. "OXIDITE").
    pub wordmark: String,
    /// Detected package/project name or folder name.
    pub name: String,
    /// Display path of the scanned root.
    pub path: String,
    pub git_branch: Option<String>,
    pub language: Option<String>,
    pub frameworks: Vec<String>,
    pub pkg_manager: Option<String>,
    pub prod_deps: Option<usize>,
    pub dev_deps: Option<usize>,
    pub files: usize,
    pub directories: usize,
    pub truncated: bool,
    pub scan_time_ms: u64,
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
    let mut frameworks = BTreeSet::new();
    let mut pkg_manager: Option<String> = None;
    let mut prod_deps: Option<usize> = None;
    let mut dev_deps: Option<usize> = None;
    let mut manifest_lang: Option<&'static str> = None;

    // 1. Inspect Cargo.toml (supports single crate or virtual workspace)
    if let Ok(cargo_toml) = fs::read_to_string(root.join("Cargo.toml")) {
        manifest_lang = Some("Rust");
        pkg_manager = Some("cargo".to_string());
        if let Some(pkg_name) = parse_toml_package_name(&cargo_toml) {
            name = pkg_name;
        }
        let (p, d) = count_cargo_deps(&cargo_toml, &mut frameworks);
        if p > 0 || d > 0 {
            prod_deps = Some(p);
            dev_deps = Some(d);
        }
    }

    // 2. Inspect root package.json or web/package.json (common Rust + Vite monorepo layout)
    let pkg_json_path = if root.join("package.json").is_file() {
        Some(root.join("package.json"))
    } else if root.join("web/package.json").is_file() {
        Some(root.join("web/package.json"))
    } else {
        None
    };

    if let Some(pkg_path) = pkg_json_path
        && let Ok(raw) = fs::read_to_string(&pkg_path)
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        if manifest_lang.is_none() {
            if let Some(n) = val.get("name").and_then(|v| v.as_str())
                && !n.is_empty()
            {
                name = n.to_string();
            }
            let pkg_dir = pkg_path.parent().unwrap_or(root);
            manifest_lang = Some(if pkg_dir.join("tsconfig.json").is_file() {
                "TypeScript"
            } else {
                "JavaScript"
            });
        }

        let js_pm = detect_js_pkg_manager(pkg_path.parent().unwrap_or(root));
        pkg_manager = match pkg_manager {
            Some(existing) => Some(format!("{existing}, {js_pm}")),
            None => Some(js_pm.to_string()),
        };

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
            detect_js_framework(key, &mut frameworks);
        }
    }

    if manifest_lang.is_none() && root.join("pyproject.toml").is_file() {
        manifest_lang = Some("Python");
        pkg_manager = Some(
            if root.join("uv.lock").is_file() {
                "uv"
            } else if root.join("poetry.lock").is_file() {
                "poetry"
            } else {
                "pip"
            }
            .to_string(),
        );
    } else if manifest_lang.is_none() && root.join("go.mod").is_file() {
        manifest_lang = Some("Go");
        pkg_manager = Some("go".to_string());
    }

    // 3. Read current Git branch without spawning a subprocess
    let git_branch = read_git_branch(root);

    // 4. Bounded directory walk for file/directory counts + fallback language detection
    let mut files = 0usize;
    let mut directories = 0usize;
    let mut truncated = false;
    let mut ext_counts: HashMap<&'static str, usize> = HashMap::new();

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
            if ft.is_dir() {
                if IGNORED_DIRS.contains(&name_str.as_ref()) {
                    continue;
                }
                directories += 1;
                if depth < MAX_SCAN_DEPTH {
                    stack.push((entry.path(), depth + 1));
                }
            } else if ft.is_file() {
                files += 1;
                if let Some(ext) = entry.path().extension().and_then(|e| e.to_str())
                    && let Some(lang) = ext_to_language(ext)
                {
                    *ext_counts.entry(lang).or_insert(0) += 1;
                }
            }
        }
    }

    let has_ts_files = ext_counts.get("TypeScript").copied().unwrap_or(0) > 0;
    let top_ext_lang = ext_counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(lang, _)| lang);

    let language = match (manifest_lang, top_ext_lang) {
        (Some("Rust"), _) if has_ts_files => Some("Rust, TypeScript".to_string()),
        (Some(m), _) => Some(m.to_string()),
        (None, Some(e)) => Some(e.to_string()),
        (None, None) => None,
    };

    let scan_time_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    Ok(ProjectReport {
        wordmark,
        name,
        path: root.to_string_lossy().into_owned(),
        git_branch,
        language,
        frameworks: frameworks.into_iter().collect(),
        pkg_manager,
        prod_deps,
        dev_deps,
        files,
        directories,
        truncated,
        scan_time_ms,
    })
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

fn detect_js_pkg_manager(dir: &Path) -> &'static str {
    if dir.join("bun.lock").is_file() || dir.join("bun.lockb").is_file() {
        "bun"
    } else if dir.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if dir.join("yarn.lock").is_file() {
        "yarn"
    } else {
        "npm"
    }
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

fn count_cargo_deps(toml: &str, frameworks: &mut BTreeSet<String>) -> (usize, usize) {
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
        if let Some((key, _)) = line.split_once('=') {
            let k = key.trim();
            if sec == Section::Deps {
                prod += 1;
            } else if sec == Section::DevDeps {
                dev += 1;
            }
            match k {
                "axum" => {
                    frameworks.insert("Axum".into());
                }
                "tokio" => {
                    frameworks.insert("Tokio".into());
                }
                "sqlx" => {
                    frameworks.insert("SQLx".into());
                }
                "actix-web" => {
                    frameworks.insert("Actix Web".into());
                }
                "ratatui" => {
                    frameworks.insert("Ratatui".into());
                }
                _ => {}
            }
        }
    }
    (prod, dev)
}

fn detect_js_framework(pkg: &str, frameworks: &mut BTreeSet<String>) {
    match pkg {
        "vite" => {
            frameworks.insert("Vite".into());
        }
        "react" => {
            frameworks.insert("React".into());
        }
        "next" => {
            frameworks.insert("Next.js".into());
        }
        "tailwindcss" => {
            frameworks.insert("Tailwind CSS".into());
        }
        "@playwright/test" | "playwright" => {
            frameworks.insert("Playwright".into());
        }
        "vitest" => {
            frameworks.insert("Vitest".into());
        }
        "svelte" => {
            frameworks.insert("Svelte".into());
        }
        "vue" => {
            frameworks.insert("Vue".into());
        }
        _ => {}
    }
}

fn ext_to_language(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Some("Rust"),
        "ts" | "tsx" => Some("TypeScript"),
        "js" | "jsx" | "mjs" => Some("JavaScript"),
        "py" => Some("Python"),
        "go" => Some("Go"),
        "c" | "h" => Some("C"),
        "cpp" | "cc" | "hpp" => Some("C++"),
        "html" => Some("HTML"),
        "css" => Some("CSS"),
        "v" | "sv" => Some("Verilog"),
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
        assert_eq!(report.language.as_deref(), Some("Rust, TypeScript"));
        assert_eq!(report.pkg_manager.as_deref(), Some("cargo, bun"));
        assert!(report.frameworks.contains(&"Axum".to_string()));
        assert!(report.frameworks.contains(&"Tokio".to_string()));
        assert!(report.frameworks.contains(&"Vite".to_string()));
        assert!(report.frameworks.contains(&"Tailwind CSS".to_string()));
        assert_eq!(report.prod_deps, Some(4)); // 3 workspace + 1 web
        assert_eq!(report.dev_deps, Some(1));
        assert_eq!(report.files, 4); // Cargo.toml, package.json, bun.lock, main.ts
        assert_eq!(report.directories, 2); // web, web/src

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
