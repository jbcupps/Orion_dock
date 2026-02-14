//! Sensory capabilities — hearing (audio), vision (video), web search, web fetch, browser.

pub mod browser;
pub mod file_ingestion;
pub mod hearing;
pub mod vision;
pub mod web_fetch;
pub mod web_search;

pub use browser::*;
pub use file_ingestion::*;
pub use hearing::*;
pub use vision::*;
pub use web_fetch::*;
pub use web_search::*;
