//! Audit log for gated tool invocations (ported from `src/main/tools/confirm.ts`).
//! confirm/danger tools run only when power-user mode is on; every gated
//! invocation — allowed, denied, or errored — is appended here for an
//! after-the-fact record. Best-effort: logging never breaks a turn.

use serde::Serialize;
