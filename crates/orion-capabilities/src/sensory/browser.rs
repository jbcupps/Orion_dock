//! Browser fetch capability trait.
//!
//! Abstraction for fetching a URL via a headless browser (e.g. Chromium CDP).
//! Implementations live in skills or other crates; this module defines the contract
//! and shared types so the router or orchestration layer can reason about browser-backed fetch.

use async_trait::async_trait;

/// Capability for fetching a URL using a real browser (JS execution, cookies, etc.).
/// Implemented by the web-browse skill when built with the `browser` feature.
#[async_trait]
pub trait BrowserFetch: Send + Sync {
    /// Fetch the page at `url` and return the document HTML after load.
    /// Caller is responsible for SSRF validation before calling.
    async fn fetch_page_html(&self, url: &str) -> Result<String, String>;
}
