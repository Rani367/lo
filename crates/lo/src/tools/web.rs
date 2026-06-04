//! Web tools: `web_search` (DuckDuckGo, no cloud AI) and `fetch_url` (a
//! SSRF-hardened page fetch). Ported from `src/main/tools/{websearch,web}.ts`.
//!
//! `fetch_url` is security-critical: only http/https, and at EVERY redirect hop
//! the host is re-validated — first the literal-host check from
//! [`lo_core::tools::ssrf::reject_literal_host`], then a DNS resolution where any
//! A/AAAA record landing in a private range (per
//! [`lo_core::tools::ssrf::is_private_ip`]) aborts the fetch. Redirects are
//! followed manually with [`reqwest::redirect::Policy::none`] so a public URL
//! that 3xx-redirects (or DNS-rebinds) to a private address can't slip past the
//! initial check.

use std::time::Duration;

use hickory_resolver::Resolver;
use lo_core::tools::ssrf::{is_private_ip, reject_literal_host};
use reqwest::redirect::Policy;
use reqwest::{Client, Url};
use scraper::{Html, Selector};

/// Browser-ish UA so DuckDuckGo's HTML endpoint serves real results.
const UA: &str = "Mozilla/5.0 (compatible; Lo/1.0)";

const MAX_BYTES: usize = 512 * 1024;
const MAX_TEXT: usize = 4000;
const MAX_REDIRECTS: usize = 5;

// --------------------------------------------------------------------------
// web_search
// --------------------------------------------------------------------------

/// Search the live web via DuckDuckGo. Tries the Instant Answer JSON API first,
/// then falls back to scraping the top HTML result snippets. Best-effort: always
/// returns a string, never errors (so the tool result is always speakable).
pub async fn web_search(query: &str) -> String {
    let q = query.trim();
    if q.is_empty() {
        return "No query was provided.".to_string();
    }

    // 1) Instant Answer API (clean JSON for many factual queries).
    if let Some(answer) = instant_answer(q).await {
        return answer;
    }

    // 2) Scrape the top HTML results.
    if let Some(snippets) = scrape_results(q).await {
        return snippets;
    }

    "I could not find anything reliable on that.".to_string()
}

async fn instant_answer(q: &str) -> Option<String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
        .ok()?;
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        urlencode(q)
    );
    let resp = client.get(url).send().await.ok()?;
    let d: serde_json::Value = resp.json().await.ok()?;

    if let Some(answer) = d.get("Answer").and_then(|v| v.as_str()) {
        if !answer.is_empty() {
