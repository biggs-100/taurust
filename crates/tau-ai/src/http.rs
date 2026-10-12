//! HTTP concerns shared by every provider adapter.
//!
//! **Deviation from the reference, deliberate.** tau's `tau_ai/http.py` mutates
//! `os.environ` around client construction to rewrite a generic `socks://` proxy
//! into the explicit `socks5://` form that httpx requires, then restores it.
//! That cannot be ported: Rust marks environment mutation as `unsafe`, this
//! workspace forbids unsafe code, and rewriting process state around a client
//! construction would race against every other thread. Normalization is instead
//! a pure function applied where the proxy is read, and the normalized value is
//! passed to the client explicitly.

/// Rewrite a generic `socks://` proxy URL into the explicit `socks5://` form.
///
/// Some environments use `socks://` as a catch-all SOCKS scheme. HTTP clients
/// generally accept explicit versions and reject the generic one, so the generic
/// form is treated as SOCKS5. Every other scheme is returned unchanged.
pub fn normalize_proxy_url(proxy_url: &str) -> String {
    const GENERIC: &str = "socks://";

    if proxy_url.len() >= GENERIC.len()
        && proxy_url.is_char_boundary(GENERIC.len())
        && proxy_url[..GENERIC.len()].eq_ignore_ascii_case(GENERIC)
    {
        return format!("socks5://{}", &proxy_url[GENERIC.len()..]);
    }

    proxy_url.to_string()
}
