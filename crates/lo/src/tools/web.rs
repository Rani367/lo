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
