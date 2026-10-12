//! Proxy URL normalization.

use tau_ai::http::normalize_proxy_url;

#[test]
fn a_generic_socks_scheme_becomes_socks5() {
    assert_eq!(
        normalize_proxy_url("socks://127.0.0.1:1080"),
        "socks5://127.0.0.1:1080"
    );
}

#[test]
fn the_scheme_check_is_case_insensitive() {
    assert_eq!(
        normalize_proxy_url("SOCKS://user:pass@proxy.local:1080"),
        "socks5://user:pass@proxy.local:1080"
    );
}

#[test]
fn explicit_schemes_are_left_alone() {
    for url in [
        "socks5://127.0.0.1:1080",
        "socks5h://127.0.0.1:1080",
        "http://127.0.0.1:8080",
        "https://proxy.example.com:8443",
    ] {
        assert_eq!(normalize_proxy_url(url), url);
    }
}
