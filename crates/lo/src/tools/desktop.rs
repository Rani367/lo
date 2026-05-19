//! Cross-platform desktop actions: open/focus/quit an app, set the system
//! volume, capture a screenshot, and report the local date+time. Ported from
//! `src/main/tools/desktop.ts`.
//!
//! Every shell-out uses an argv array (never a shell string) so an
//! LLM-provided name can't inject. On Windows the app name is handed to
//! PowerShell through the `LO_APP` environment variable — never interpolated
