# Security Policy

## Reporting a vulnerability

If you discover a security vulnerability in Lo, please report it privately rather
than opening a public issue. Email **rani2011367@gmail.com** with a description,
reproduction steps, and the affected version. You can expect an acknowledgement
and, where applicable, a fix in a subsequent release.

## Supported versions

The latest `1.x` release receives security fixes.

## Lo's security model

Lo runs entirely on your machine and makes no cloud calls for its core function,
which removes whole classes of remote risk. The deliberate safeguards are:

- **Tiered tool safety gate.** Every tool is classified `safe`, `confirm`, or
  `danger`. The `confirm`/`danger` tiers (writing/moving/copying/deleting files,
  running commands, quitting/opening apps) are refused unless `powerUserMode` is
  enabled in settings, and every gated invocation — allowed, denied, or errored —
  is recorded in a rotating audit log.
- **Filesystem sandbox.** The file tools resolve every path (expanding `~` and
  following symlinks) and require it to stay within the configured `allowedFsRoots`
  (your home directory by default), so the model cannot escape to arbitrary paths.
- **SSRF guard.** `web_search` / `fetch_url` accept only public `http`/`https`
  hosts. Private, loopback, link-local, ULA, CGNAT, multicast, and cloud-metadata
  addresses are rejected; the classification is case-insensitive for IPv4-mapped
  IPv6 (RFC 5952); every redirect hop is re-validated; and DNS results are checked
  against **all** resolved records, not just the first.
- **Shell safety.** `run_command` takes an executable plus an argument vector (never
  a shell string), runs with a timeout, and caps captured output.

## A note on release artifacts

Release installers are currently **unsigned**. On macOS, right-click the app and
choose **Open** the first time (or clear the quarantine attribute); on Windows,
SmartScreen may warn before you can run the installer. Signing/notarization slots
are wired into the release pipeline and can be enabled when certificates are
available.
