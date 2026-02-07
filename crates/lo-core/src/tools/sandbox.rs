//! Filesystem sandbox (ported from `resolveInRoots`/`allowedRoots` in
//! `src/main/tools/files.ts`). Every path the model supplies is expanded,
//! absolutized, lexically normalized, then realpath'd (longest existing prefix)
//! and verified to live inside an allowed root before any I/O happens — so a
