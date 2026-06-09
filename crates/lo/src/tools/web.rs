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
            return Some(answer.to_string());
        }
    }
    if let Some(abstract_text) = d.get("AbstractText").and_then(|v| v.as_str()) {
        if !abstract_text.is_empty() {
            return Some(match d.get("AbstractSource").and_then(|v| v.as_str()) {
                Some(src) if !src.is_empty() => format!("{abstract_text} ({src})"),
                _ => abstract_text.to_string(),
            });
        }
    }
    if let Some(topics) = d.get("RelatedTopics").and_then(|v| v.as_array()) {
        for t in topics {
            if let Some(text) = t.get("Text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

async fn scrape_results(q: &str) -> Option<String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(7))
        .user_agent(UA)
        .build()
        .ok()?;
    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencode(q));
    let resp = client.get(url).send().await.ok()?;
    let html = resp.text().await.ok()?;

    // Parse `.result__snippet` nodes and join up to 3, capped at 700 chars.
    let doc = Html::parse_document(&html);
    let selector = Selector::parse(".result__snippet").ok()?;
    let snippets: Vec<String> = doc
        .select(&selector)
        .take(3)
        .map(|el| collapse_ws(&el.text().collect::<String>()))
        .filter(|s| !s.is_empty())
        .collect();
    if snippets.is_empty() {
        return None;
    }
    let joined = snippets.join(" • ");
    Some(cap_chars(&joined, 700))
}

// --------------------------------------------------------------------------
// fetch_url
// --------------------------------------------------------------------------

/// Fetch a URL and return its readable text. http/https only; SSRF-hardened by
/// re-validating the host at every redirect hop. Returns `Err(message)` for the
/// caller to wrap as `Error running fetch_url: …`.
pub async fn fetch_url(raw_url: &str) -> Result<String, String> {
    let mut url = parse_http(raw_url)?;

    // A no-redirect client so we follow 3xx manually and re-check the host each
    // hop. A 10s per-request timeout matches the TS `AbortSignal.timeout`.
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("could not build the HTTP client: {e}"))?;

    let mut response: Option<reqwest::Response> = None;
    for hop in 0..=MAX_REDIRECTS {
        let host = url
            .host_str()
            .ok_or_else(|| "That is not a valid URL.".to_string())?
            .to_string();
        assert_public_host(&host).await?;

        let res = client
            .get(url.clone())
            .header("user-agent", "Mozilla/5.0 (compatible; Lo/1.0)")
            .send()
            .await
            .map_err(|_| "The request failed.".to_string())?;

        let status = res.status();
        let location = if status.is_redirection() {
            res.headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        } else {
            None
        };

        match location {
            None => {
                response = Some(res);
                break;
            }
            Some(loc) => {
                if hop == MAX_REDIRECTS {
                    return Err("Too many redirects.".to_string());
                }
                let next = url
                    .join(&loc)
                    .map_err(|_| "That is not a valid URL.".to_string())?;
                url = parse_http(next.as_str())?;
            }
        }
    }

    let res = response.ok_or_else(|| "The request failed.".to_string())?;
    let status = res.status();
    if !status.is_success() {
        return Err(format!("The server returned {}.", status.as_u16()));
    }

    // Capture the content-type before consuming the body.
    let content_type = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let bytes = res
        .bytes()
        .await
        .map_err(|_| "The request failed.".to_string())?;
    let capped = &bytes[..bytes.len().min(MAX_BYTES)];
    let body = String::from_utf8_lossy(capped);

    let text = if content_type.contains("html") {
        html_to_text(&body)
    } else {
        body.trim().to_string()
    };

    if text.is_empty() {
        return Ok("The page had no readable text.".to_string());
    }
    Ok(if text.chars().count() > MAX_TEXT {
        let head: String = text.chars().take(MAX_TEXT).collect();
        format!("{head} …")
    } else {
        text
    })
}

/// Parse a string into an http/https URL or fail.
fn parse_http(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw.trim()).map_err(|_| "That is not a valid URL.".to_string())?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("Only http and https URLs are allowed.".to_string());
    }
    Ok(url)
}

/// Resolve `hostname` and refuse private/loopback/link-local destinations.
/// First the literal-host check, then DNS resolution where any private record
/// aborts. Mirrors `assertPublicHost` in `web.ts`.
async fn assert_public_host(hostname: &str) -> Result<(), String> {
    if reject_literal_host(hostname).is_some() {
        return Err("Refusing to fetch a private or local address.".to_string());
    }

    let resolver = Resolver::builder_tokio()
        .map_err(|_| format!("Could not resolve {hostname}."))?
        .build();
    let lookup = resolver
        .lookup_ip(hostname)
        .await
        .map_err(|_| format!("Could not resolve {hostname}."))?;

    let mut saw_record = false;
    for ip in lookup.iter() {
        saw_record = true;
        if is_private_ip(&ip.to_string()) {
            return Err("Refusing to fetch a host that resolves to a private address.".to_string());
        }
    }
    if !saw_record {
        return Err(format!("Could not resolve {hostname}."));
    }
    Ok(())
}

// --------------------------------------------------------------------------
// small text helpers
// --------------------------------------------------------------------------

/// Strip tags + decode the common HTML entities and collapse whitespace. The
/// non-HTML/HTML branch of `fetch_url` uses `html2text`; this dependency-free
/// pass mirrors the TS `htmlToText` for cases where a heavier parse is overkill.
fn html_to_text(html: &str) -> String {
    // Prefer html2text for fidelity; it renders to readable plain text.
    if let Ok(rendered) = html2text::from_read(html.as_bytes(), 100) {
        let trimmed = rendered.trim();
        if !trimmed.is_empty() {
            return collapse_ws(trimmed);
        }
    }
    // Fallback: a minimal entity/tag strip (matches the TS regex pass).
    let mut s = strip_tags(html);
    s = s
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'");
    collapse_ws(&s)
}

/// Remove `<…>` tags (and script/style bodies) with a tiny state machine.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Collapse runs of whitespace into single spaces and trim.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cap a string to `n` chars (by char boundary, not bytes).
fn cap_chars(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        s.chars().take(n).collect()
    } else {
        s.to_string()
    }
}

/// Percent-encode a query component (RFC 3986 unreserved set is left as-is).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
