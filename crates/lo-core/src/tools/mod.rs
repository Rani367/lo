//! Tool registry + safety gate (ported from `src/main/tools/registry.ts`).
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

/// The tier for a tool name (unknown names are treated as `Safe`, matching the TS
/// `?? 'safe'`).
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

// --- schema builders mirroring the TS `str`/`obj` helpers ---

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
