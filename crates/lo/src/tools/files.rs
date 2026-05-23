//! Filesystem tools — read/list/search/open/write/move/delete, all sandboxed to
//! the allowed roots. Ported from `src/main/tools/files.ts`. Every path the
//! model supplies goes through [`lo_core::tools::sandbox::resolve_in_roots`],
//! which expands `~`, absolutizes, lexically normalizes, realpath-dereferences
//! symlinks, and verifies the result lives inside an allowed root — so a
