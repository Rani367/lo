//! Tool registry + safety gate: every advertised tool carries a safety tier, and
//! the gate is enforced here before a tool runs.
//!
//! Each tool carries a safety `tier`:
//!   - `Safe`    — read-only or trivially reversible; runs immediately.
//!   - `Confirm` — a visible, reversible side effect; gated unless power-user mode.
//!   - `Danger`  — destructive / irreversible / arbitrary code; gated unless power-user.
//!
//! The gate is enforced HERE, before a tool runs — the model is never trusted to
//! police itself, and every gated invocation is audit-logged. The JSON schemas
//! are the exact ones advertised to the brain via the API `tools[]` parameter.
//!
//! Tool *execution* bodies (which need HTTP, process spawning, clipboard, etc.)
//! live in the `lo` binary crate; the registry, the schemas, the gate, the SSRF
//! guard, the filesystem sandbox, the argv validation, and the audit log are all
//! here and unit-tested.

pub mod audit;
pub mod sandbox;
pub mod shell;
pub mod ssrf;

use serde::Serialize;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Safe,
    Confirm,
    Danger,
}

/// One advertised tool: its safety tier plus the OpenAI function schema.
pub struct ToolSchema {
    pub tier: Tier,
    pub name: &'static str,
    pub description: &'static str,
    /// JSON-schema `parameters` object.
    pub parameters: serde_json::Value,
}

/// The canned refusal returned when a gated tool is called with power-user off.
pub const POWER_USER_REQUIRED: &str =
    "That action needs power-user mode, which is off. Enable powerUserMode in settings to allow it.";

/// The result of the safety gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// Run the tool.
    Allow,
    /// Refuse: a non-safe tool with power-user mode off.
    DenyNeedsPowerUser,
}

static REGISTRY: LazyLock<Vec<ToolSchema>> = LazyLock::new(build_registry);

/// The full advertised tool set.
pub fn tool_schemas() -> &'static [ToolSchema] {
    &REGISTRY
}

/// The tools serialized for the `tools[]` request parameter (tier omitted — it's
/// an internal concern the model never sees).
pub fn tool_schemas_json() -> serde_json::Value {
    serde_json::Value::Array(
        REGISTRY
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect(),
    )
}

/// Compact comma-separated tool-name list for the persona prompt.
pub fn tool_names() -> String {
    REGISTRY
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The tier for a tool name (unknown names are treated as `Safe`).
pub fn tier_for(name: &str) -> Tier {
    REGISTRY
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.tier)
        .unwrap_or(Tier::Safe)
}

/// The safety gate: confirm/danger tools run only when power-user mode is on.
pub fn gate(name: &str, power_user_mode: bool) -> GateDecision {
    if tier_for(name) != Tier::Safe && !power_user_mode {
        GateDecision::DenyNeedsPowerUser
    } else {
        GateDecision::Allow
    }
}

// --- schema builders: small helpers for the common string / object param shapes ---

fn str_param(desc: &str) -> serde_json::Value {
    serde_json::json!({ "type": "string", "description": desc })
}

fn obj(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": properties, "required": required })
}

fn tool(
    tier: Tier,
    name: &'static str,
    description: &'static str,
    parameters: serde_json::Value,
) -> ToolSchema {
    ToolSchema {
        tier,
        name,
        description,
        parameters,
    }
}

fn build_registry() -> Vec<ToolSchema> {
    use Tier::*;
    vec![
        // ---- information / web ----
        tool(Safe, "web_search", "Search the live web for current, factual, or time-sensitive information (news, weather, prices, scores, events, lookups). Use whenever you are not certain of the answer from memory.",
            obj(serde_json::json!({ "query": str_param("A focused natural-language search query.") }), &["query"])),
        tool(Safe, "fetch_url", "Fetch a web page (or API URL) and return its readable text so you can answer about its contents. Public http/https only.",
            obj(serde_json::json!({ "url": str_param("The absolute http(s) URL to fetch.") }), &["url"])),
        tool(Safe, "get_datetime", "Get the current local date and time.", obj(serde_json::json!({}), &[])),
        tool(Safe, "system_info", "Read host telemetry: overview, cpu, memory, disk, battery, network, or all.",
            obj(serde_json::json!({ "kind": str_param("One of: overview, cpu, memory, disk, battery, network, all.") }), &[])),

        // ---- apps / desktop ----
        tool(Safe, "open_app", "Open/launch an application on the computer by name (e.g. \"Spotify\", \"Safari\").",
            obj(serde_json::json!({ "name": str_param("The application name.") }), &["name"])),
        tool(Safe, "focus_app", "Bring an already-running application to the foreground by name.",
            obj(serde_json::json!({ "name": str_param("The application name.") }), &["name"])),
        tool(Confirm, "quit_app", "Quit a running application by name.",
            obj(serde_json::json!({ "name": str_param("The application name.") }), &["name"])),
        tool(Safe, "set_volume", "Set the system output volume to a percentage from 0 to 100.",
            obj(serde_json::json!({ "percent": { "type": "number", "description": "0-100" } }), &["percent"])),
        tool(Safe, "media_control", "Control media playback: play, pause, playpause, next, previous, or stop.",
            obj(serde_json::json!({ "action": str_param("play | pause | playpause | next | previous | stop") }), &["action"])),
        tool(Safe, "set_timer", "Start a countdown timer; Lo announces when it completes.",
            obj(serde_json::json!({ "seconds": { "type": "number", "description": "Duration in seconds." }, "label": str_param("Optional label, e.g. \"tea\".") }), &["seconds"])),
        tool(Safe, "take_screenshot", "Capture a screenshot of the screen and save it to the Pictures folder.", obj(serde_json::json!({}), &[])),

        // ---- clipboard ----
        tool(Safe, "read_clipboard", "Read the current text contents of the system clipboard.", obj(serde_json::json!({}), &[])),
        tool(Safe, "write_clipboard", "Replace the system clipboard with the given text.",
            obj(serde_json::json!({ "text": str_param("The text to copy.") }), &["text"])),

        // ---- filesystem (sandboxed to the allowed roots) ----
        tool(Safe, "read_file", "Read a text file (within the allowed folders) and return its contents.",
            obj(serde_json::json!({ "path": str_param("Absolute or ~ path to the file.") }), &["path"])),
        tool(Safe, "list_dir", "List the contents of a directory (within the allowed folders).",
            obj(serde_json::json!({ "path": str_param("Absolute or ~ path to the directory.") }), &["path"])),
        tool(Safe, "search_files", "Find files whose names contain a query, under a directory (within the allowed folders).",
            obj(serde_json::json!({ "path": str_param("Directory to search under."), "query": str_param("Substring to match in file names.") }), &["path", "query"])),
        tool(Confirm, "open_path", "Open a file or folder in its default application / the file manager.",
            obj(serde_json::json!({ "path": str_param("Absolute or ~ path.") }), &["path"])),
        tool(Danger, "write_file", "Create or overwrite a text file with the given contents (within the allowed folders).",
            obj(serde_json::json!({ "path": str_param("Absolute or ~ path."), "content": str_param("The file contents."), "overwrite": { "type": "boolean", "description": "Allow replacing an existing file." } }), &["path", "content"])),
        tool(Danger, "move_path", "Move or rename a file or folder (within the allowed folders).",
            obj(serde_json::json!({ "from": str_param("Source path."), "to": str_param("Destination path.") }), &["from", "to"])),
        tool(Danger, "copy_file", "Copy a file to a new path (within the allowed folders).",
            obj(serde_json::json!({ "from": str_param("Source file path."), "to": str_param("Destination path.") }), &["from", "to"])),
        tool(Danger, "delete_path", "Delete a file or folder (within the allowed folders).",
            obj(serde_json::json!({ "path": str_param("Absolute or ~ path.") }), &["path"])),

        // ---- shell ----
        tool(Danger, "run_command", "Run a shell command. Provide the executable and its arguments as separate values (never one string). Use for anything no other tool covers.",
            obj(serde_json::json!({ "command": str_param("The executable, e.g. \"git\"."), "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments, e.g. [\"status\"]." }, "cwd": str_param("Optional working directory (within the allowed folders).") }), &["command"])),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_all_22_tools() {
        assert_eq!(tool_schemas().len(), 22);
    }

    #[test]
    fn safety_tiers_are_correct() {
        assert_eq!(tier_for("web_search"), Tier::Safe);
        assert_eq!(tier_for("read_file"), Tier::Safe);
        assert_eq!(tier_for("quit_app"), Tier::Confirm);
        assert_eq!(tier_for("open_path"), Tier::Confirm);
        assert_eq!(tier_for("write_file"), Tier::Danger);
        assert_eq!(tier_for("move_path"), Tier::Danger);
        assert_eq!(tier_for("copy_file"), Tier::Danger);
        assert_eq!(tier_for("delete_path"), Tier::Danger);
        assert_eq!(tier_for("run_command"), Tier::Danger);
        // Unknown tools default to Safe.
        assert_eq!(tier_for("nonexistent_tool"), Tier::Safe);
    }

    #[test]
    fn gate_refuses_danger_without_power_user() {
        assert_eq!(gate("run_command", false), GateDecision::DenyNeedsPowerUser);
        assert_eq!(gate("delete_path", false), GateDecision::DenyNeedsPowerUser);
        assert_eq!(gate("quit_app", false), GateDecision::DenyNeedsPowerUser);
        // Safe tools always run.
        assert_eq!(gate("web_search", false), GateDecision::Allow);
        assert_eq!(gate("read_file", false), GateDecision::Allow);
    }

    #[test]
    fn gate_allows_everything_in_power_user_mode() {
        for t in tool_schemas() {
            assert_eq!(
                gate(t.name, true),
                GateDecision::Allow,
                "{} should be allowed",
                t.name
            );
        }
    }

    #[test]
    fn advertised_json_omits_tier_and_keeps_schema() {
        let json = tool_schemas_json();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 22);
        let first = &arr[0];
        assert_eq!(first["type"], "function");
        assert!(first["function"]["name"].is_string());
        assert!(first["function"].get("tier").is_none());
        assert!(first["function"]["parameters"]["type"] == "object");
    }

    #[test]
    fn tool_names_lists_everything() {
        let names = tool_names();
        assert!(names.contains("web_search"));
        assert!(names.contains("run_command"));
        assert!(names.contains("copy_file"));
        assert_eq!(names.split(", ").count(), 22);
    }
}
