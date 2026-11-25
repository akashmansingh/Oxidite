//! Route classification and cache policy for gateway responses.

use std::path::{Component, Path, PathBuf};

/// A cache policy class independent of user or session identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RouteClass {
    /// A content-hashed static asset that may be cached indefinitely.
    FingerprintedAsset,
    /// The HTML application shell.
    Html,
    /// An authenticated or state-changing HTTP API route.
    Api,
    /// Operational endpoints and WebSocket upgrades.
    Metrics,
    /// Static content without a content hash.
    Static,
}

impl RouteClass {
    /// Returns the low-cardinality metric label for this route class.
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            RouteClass::FingerprintedAsset => "fingerprinted_asset",
            RouteClass::Html => "html",
            RouteClass::Api => "api",
            RouteClass::Metrics => "metrics",
            RouteClass::Static => "static",
        }
    }
}

/// Classifies a request path without retaining the path as metric data.
#[must_use]
pub fn classify_request_path(path: &str) -> RouteClass {
    if path.starts_with("/api/") || path == "/api" {
        RouteClass::Api
    } else if matches!(path, "/metrics" | "/health" | "/ready" | "/ws") {
        RouteClass::Metrics
    } else if path == "/" || path.ends_with(".html") {
        RouteClass::Html
    } else if is_fingerprinted_asset(path) {
        RouteClass::FingerprintedAsset
    } else {
        RouteClass::Static
    }
}

/// Returns the cache policy for a route class.
#[must_use]
pub const fn cache_control(class: RouteClass) -> &'static str {
    match class {
        RouteClass::FingerprintedAsset => "public, max-age=31536000, immutable",
        RouteClass::Html | RouteClass::Static => "no-cache",
        RouteClass::Api | RouteClass::Metrics => "no-store",
    }
}

/// Resolves a static asset only when every path component is a normal child.
#[must_use]
pub fn resolve_static_path(static_dir: &Path, request_path: &str) -> Option<PathBuf> {
    let relative = Path::new(request_path);
    if relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Some(static_dir.join(relative))
    } else {
        None
    }
}

fn is_fingerprinted_asset(path: &str) -> bool {
    let Some(file_name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let stem = file_name.split('.').next().unwrap_or_default();
    stem.match_indices('-').any(|(idx, _)| {
        if idx == 0 {
            return false;
        }
        let hash = &stem[idx + 1..];
        hash.len() >= 8
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    })
}
