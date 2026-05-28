//! OS-action tool *execution* bodies — the side-effecting half of the tool
//! system. The registry, schemas, safety gate, SSRF classifier, filesystem
//! sandbox, argv validation, and audit log all live in [`lo_core::tools`] and are
//! reused verbatim here; this module only carries the bodies that need HTTP,
//! process spawning, the clipboard, screen capture, notifications, and timers.
//!
//! Ported from `src/main/tools/{web,websearch,system,desktop,media,files,shell,
//! clipboard}.ts`. The [`dispatch`] entry reproduces `registry.ts`'s
//! `dispatchTool` exactly: parse args, run the safety gate, audit non-`Safe`
//! tiers, and return a plain string the brain can phrase in the Lo voice.

mod clipboard;
mod desktop;
mod files;
mod media;
mod shell;
mod system;
mod timer;
mod web;

use lo_core::tools::audit::{self, Decision};
use lo_core::tools::{self, GateDecision};
use lo_core::LoSettings;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

/// Execute a requested tool by name and return a string result for the brain.
///
/// Mirrors `dispatchTool` in `registry.ts`:
/// 1. Parse `args_json` with serde_json; on parse failure return a fixed message.
/// 2. Run the safety gate; if it denies, audit `Denied` and return the canned
///    power-user refusal.
/// 3. Otherwise execute. For non-`Safe` tiers, audit `Allowed` (with the result)
///    on success or `Error` (with the message) on failure.
///
/// `announce` is the worker → UI string channel a fired timer speaks into.
pub async fn dispatch(
    name: &str,
    args_json: &str,
    settings: &LoSettings,
    announce: &UnboundedSender<String>,
) -> String {
    // 1) Parse the arguments (empty string => empty object, matching the TS).
    let args: Value = if args_json.is_empty() {
        Value::Object(Default::default())
    } else {
        match serde_json::from_str::<Value>(args_json) {
            Ok(v) => v,
            Err(_) => return format!("Error: could not parse arguments for {name}."),
        }
    };

    // 2) Safety gate: confirm/danger tools run only with power-user mode on.
    let tier = tools::tier_for(name);
    let safe = tier == tools::Tier::Safe;
    if let GateDecision::DenyNeedsPowerUser = tools::gate(name, settings.power_user_mode) {
        audit::audit_log(name, &args, Decision::Denied, "");
        return tools::POWER_USER_REQUIRED.to_string();
    }

    // 3) Execute, then audit the outcome for non-safe tiers.
    match execute(name, &args, settings, announce).await {
        Ok(result) => {
            if !safe {
                audit::audit_log(name, &args, Decision::Allowed, &result);
            }
            result
        }
        Err(message) => {
            if !safe {
                audit::audit_log(name, &args, Decision::Error, &message);
            }
            format!("Error running {name}: {message}")
        }
    }
}

/// String helper mirroring `String(args.x ?? '')`: returns the string at `key`,
/// or `""` if missing/non-string.
fn str_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Number helper mirroring `Number(args.x)`: accepts a JSON number or a numeric
/// string, else NaN-like behavior (returns `f64::NAN`, callers clamp/round).
fn num_arg(args: &Value, key: &str) -> f64 {
    match args.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

/// `args.x ? String(args.x) : undefined` — a present, non-empty string or None.
fn opt_str_arg(args: &Value, key: &str) -> Option<String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// `Array.isArray(v) ? v.map(String) : []` — coerce a JSON array to `Vec<String>`.
fn string_array_arg(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                Value::Null => "null".to_string(),
                other => other.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The `switch (name)` body from `registry.ts`'s `execute`. Returns `Ok(result)`
/// or `Err(message)`; the caller wraps the error as `Error running {name}: ...`.
async fn execute(
    name: &str,
    args: &Value,
    settings: &LoSettings,
    announce: &UnboundedSender<String>,
) -> Result<String, String> {
    match name {
        // ---- information / web ----
        "web_search" => Ok(web::web_search(&str_arg(args, "query")).await),
        "fetch_url" => web::fetch_url(&str_arg(args, "url")).await,
        "get_datetime" => Ok(desktop::get_datetime()),
        "system_info" => {
            let kind = args
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("overview");
            Ok(system::system_info(kind, settings).await)
        }

        // ---- apps / desktop ----
        "open_app" => desktop::open_app(&str_arg(args, "name")).await,
        "focus_app" => desktop::focus_app(&str_arg(args, "name")).await,
        "quit_app" => desktop::quit_app(&str_arg(args, "name")).await,
        "set_volume" => desktop::set_volume(num_arg(args, "percent")).await,
        "media_control" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("playpause");
            media::media_control(action).await
        }
        "set_timer" => Ok(timer::set_timer(
            num_arg(args, "seconds"),
            opt_str_arg(args, "label"),
            announce.clone(),
        )),
        "take_screenshot" => desktop::take_screenshot().await,

        // ---- clipboard ----
        "read_clipboard" => clipboard::read_clipboard(),
