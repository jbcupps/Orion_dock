//! Chromium CDP backend for fetching JS-rendered pages.
//! Compiled only when feature "browser" is enabled.

use chromiumoxide::browser::{Browser, BrowserConfig};
use futures_util::StreamExt;
use std::time::Duration;

/// Navigation timeout for browser.
const NAVIGATION_TIMEOUT_SECS: u64 = 25;

/// Fetch a URL via headless Chromium and return the document HTML.
/// Caller must have validated the URL (SSRF) already.
pub async fn fetch_page_html(url: &str) -> Result<String, String> {
    let config = BrowserConfig::builder()
        .no_sandbox()
        .build()
        .map_err(|e| format!("Browser config: {}", e))?;

    let (browser, mut handler) = Browser::launch(config).await.map_err(|e| {
        format!(
            "Launch Chrome failed (is Chromium/Chrome installed?): {}",
            e
        )
    })?;

    let handle = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let result = async {
        let page = browser
            .new_page(url)
            .await
            .map_err(|e| format!("New page failed: {}", e))?;

        page.set_default_navigation_timeout(Duration::from_secs(NAVIGATION_TIMEOUT_SECS))
            .await;

        page.goto(url)
            .await
            .map_err(|e| format!("Navigation failed: {}", e))?;

        page.wait_for_navigation()
            .await
            .map_err(|e| format!("Wait for load failed: {}", e))?;

        let html = page
            .content()
            .await
            .map_err(|e| format!("Get content failed: {}", e))?;

        browser.close().await.ok();
        Ok(html)
    }
    .await;

    handle.abort();

    result
}
