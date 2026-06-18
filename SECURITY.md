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

### Verifying your download

Even unsigned, every release is verifiable for free:

- **Checksums.** Each release attaches a `SHA256SUMS` file. After downloading,
  confirm the installer matches:

  ```bash
  # macOS / Linux (run in the folder with the installer + SHA256SUMS)
  shasum -a 256 -c SHA256SUMS
  ```

  ```powershell
  # Windows PowerShell
  Get-FileHash .\Lo_1.0.0_x64-setup.exe -Algorithm SHA256
  ```

- **Signed checksums (GPG).** `SHA256SUMS` is GPG-signed as `SHA256SUMS.asc` with
  the project's release key, published in [`KEYS`](KEYS)
  (fingerprint `1CAC E82E 23D3 4B4F 4318  6870 1310 4184 FA59 1EFF`). Confirm the
  checksum file is authentic before trusting it:

  ```bash
  gpg --import KEYS
  gpg --verify SHA256SUMS.asc SHA256SUMS
  ```

- **Build provenance.** Every installer carries a signed
  [SLSA build-provenance attestation](https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations/using-artifact-attestations-to-establish-provenance-for-builds)
  proving it was built by this repository's CI from this source. Verify it with
  the GitHub CLI:

  ```bash
  gh attestation verify <installer> --repo Rani367/lo
  ```
