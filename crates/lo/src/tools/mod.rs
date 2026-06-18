//! OS-action tool *execution* bodies — the side-effecting half of the tool
//! system. The registry, schemas, safety gate, SSRF classifier, filesystem
//! sandbox, argv validation, and audit log all live in [`lo_core::tools`] and are
//! reused verbatim here; this module only carries the bodies that need HTTP,
//! process spawning, the clipboard, screen capture, notifications, and timers.
//!
//! The [`dispatch`] entry implements the tool dispatcher for the registry: parse
//! args, run the safety gate, audit non-`Safe` tiers, and return a plain string
//! the brain can phrase in the Lo voice.

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

/// A compact local timestamp used to ground the brain's system context.
pub use desktop::datetime_context;

/// Execute a requested tool by name and return a string result for the brain.
///
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
    // 1) Parse the arguments (empty string => empty object).
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

/// Coerce the value at `key` to a string: returns the string at `key`, or `""`
/// if missing/non-string.
fn str_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Coerce the value at `key` to a number: accepts a JSON number or a numeric
/// string, else NaN-like behavior (returns `f64::NAN`, callers clamp/round).
fn num_arg(args: &Value, key: &str) -> f64 {
    match args.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

/// A present, non-empty string at `key`, or `None`.
fn opt_str_arg(args: &Value, key: &str) -> Option<String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Coerce a JSON array at `key` to `Vec<String>` (stringifying each element),
/// or an empty vec if the value is missing or not an array.
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

/// Dispatch by tool name to the matching execution body. Returns `Ok(result)`
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
        "write_clipboard" => clipboard::write_clipboard(&str_arg(args, "text")),

        // ---- filesystem ----
        "read_file" => files::read_file(settings, &str_arg(args, "path")).await,
        "list_dir" => files::list_dir(settings, &str_arg(args, "path")).await,
        "search_files" => {
            files::search_files(settings, &str_arg(args, "path"), &str_arg(args, "query")).await
        }
        "open_path" => files::open_path(settings, &str_arg(args, "path")).await,
        "write_file" => {
            files::write_file(
                settings,
                &str_arg(args, "path"),
                &str_arg(args, "content"),
                args.get("overwrite")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
            .await
        }
        "move_path" => {
            files::move_path(settings, &str_arg(args, "from"), &str_arg(args, "to")).await
        }
        "copy_file" => {
            files::copy_file(settings, &str_arg(args, "from"), &str_arg(args, "to")).await
        }
        "delete_path" => files::delete_path(settings, &str_arg(args, "path")).await,

        // ---- shell ----
        "run_command" => {
            shell::run_command(
                settings,
                &str_arg(args, "command"),
                &string_array_arg(args, "args"),
                opt_str_arg(args, "cwd").as_deref(),
            )
            .await
        }

        // Unknown tools return a normal string (an `Ok`, not an `Err`, so it is
        // not wrapped as "Error running …").
        other => Ok(format!("Error: unknown tool \"{other}\".")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A live sender whose receiver is dropped — tools only `send` best-effort.
    fn announce() -> UnboundedSender<String> {
        tokio::sync::mpsc::unbounded_channel().0
    }

    fn settings_with_root(root: &Path) -> LoSettings {
        let canon = std::fs::canonicalize(root).unwrap();
        LoSettings {
            allowed_fs_roots: vec![canon.to_string_lossy().into_owned()],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn malformed_json_args_are_reported() {
        let out = dispatch(
            "get_datetime",
            "{not json",
            &LoSettings::default(),
            &announce(),
        )
        .await;
        assert!(out.contains("could not parse arguments"), "{out}");
    }

    #[tokio::test]
    async fn unknown_tool_is_reported() {
        let out = dispatch("no_such_tool", "{}", &LoSettings::default(), &announce()).await;
        assert!(out.contains("unknown tool"), "{out}");
    }

    #[tokio::test]
    async fn danger_tool_denied_without_power_user() {
        let s = LoSettings {
            power_user_mode: false,
            ..Default::default()
        };
        let args = serde_json::json!({ "path": "x", "content": "y" }).to_string();
        let out = dispatch("write_file", &args, &s, &announce()).await;
        assert_eq!(out, lo_core::tools::POWER_USER_REQUIRED);
    }

    #[tokio::test]
    async fn safe_read_through_dispatch_inside_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let p = dir.path().join("hi.txt");
        std::fs::write(&p, b"sandboxed hello").unwrap();
        let args = serde_json::json!({ "path": p.to_string_lossy() }).to_string();
        let out = dispatch("read_file", &args, &s, &announce()).await;
        assert_eq!(out, "sandboxed hello");
    }

    #[tokio::test]
    async fn read_through_dispatch_rejects_sandbox_escape() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let args = serde_json::json!({ "path": "../../etc/hosts" }).to_string();
        let out = dispatch("read_file", &args, &s, &announce()).await;
        assert!(out.starts_with("Error running read_file:"), "{out}");
    }
}
