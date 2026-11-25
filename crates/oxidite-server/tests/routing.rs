use axum::http::header;
use oxidite_server::routing::{
    RouteClass, cache_control, classify_request_path, resolve_static_path,
};
use std::path::Path;

#[test]
fn fingerprinted_assets_are_immutable_but_html_and_private_routes_are_not() {
    assert_eq!(
        classify_request_path("/assets/app-0123456789abcdef.js"),
        RouteClass::FingerprintedAsset
    );
    assert_eq!(
        cache_control(RouteClass::FingerprintedAsset),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(classify_request_path("/"), RouteClass::Html);
    assert_eq!(cache_control(RouteClass::Html), "no-cache");
    assert_eq!(classify_request_path("/api/sessions"), RouteClass::Api);
    assert_eq!(cache_control(RouteClass::Api), "no-store");
    assert_eq!(classify_request_path("/metrics"), RouteClass::Metrics);
    assert_eq!(cache_control(RouteClass::Metrics), "no-store");
}

#[test]
fn static_resolution_rejects_parent_components() {
    let public = Path::new("web/dist");
    assert!(resolve_static_path(public, "assets/app-0123456789abcdef.js").is_some());
    assert!(resolve_static_path(public, "../Cargo.toml").is_none());
    assert!(resolve_static_path(public, "assets/../../Cargo.toml").is_none());
    assert!(resolve_static_path(public, ".env").is_none());
    assert!(resolve_static_path(public, ".git/config").is_none());
}

#[test]
fn cache_control_is_a_valid_http_header_value() {
    let header_value = axum::http::HeaderValue::from_static(cache_control(RouteClass::Api));
    assert_eq!(header_value, "no-store");
    assert_eq!(header::CACHE_CONTROL, "cache-control");
}
