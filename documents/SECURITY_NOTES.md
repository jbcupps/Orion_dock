# Security Notes

## Key Management

### External Signing Keypair (Ed25519)
- **Generated at first run** by the user's Orion instance
- **Private key is shown ONCE** during initial setup - user MUST save it securely
- **Private key is NEVER stored** by Orion - only the public key is retained
- **Public key location:** `{data_dir}/external_pubkey.bin` (auto-detected)
- **Purpose:** Signs constitutional documents (soul.md, ethics.md, instincts.md)

### Internal Keyring (Mentor Keypair)
- **Generated automatically** at first run
- **DPAPI-protected** on Windows (user scope), plaintext stub on other platforms (dev only)
- **Purpose:** Internal operations (signing memories, etc.)

## Storage Security

- **DPAPI:** Keyring and email passwords use Windows DPAPI when available
- **Non-Windows:** Plaintext stub with warning logged at startup (for development only). Do not use for production on macOS/Linux until a cross-platform secret store is integrated.
- **Keys file:** `{data_dir}/keys.bin` (DPAPI-encrypted)

## Secrets Handling

- **No secrets in repo:** API keys, passwords never committed
- **If any local config was ever committed or exposed** (e.g. `.claude/settings.local.json`), rotate all credentials referenced there (email, API keys, passwords) immediately.
- **Environment:** Use `example.env` as template; `.env` is gitignored
- **Email passwords:** Encrypted via DPAPI before storage in config
- **Secret key namespace:** `store_secret` / `check_secret` / `remove_secret` accept only (1) reserved provider names: `openai`, `anthropic`, `xai`, `google`, `tavily`, or (2) secret names declared in a skill’s `skill.toml` (under `secrets[].name`). Other keys are rejected to avoid overwriting provider keys or polluting the vault.
- **Logging:** Logs must not contain API keys, passwords, or other secrets. User-controlled paths (e.g. backup destination) are not logged in full. HTTP clients used for API key validation do not log request URL or body.

## Constitutional Document Integrity

- **Signed at first run:** soul.md, ethics.md, instincts.md are signed when keypair is generated
- **Verified at every boot:** Signatures checked against the stored public key
- **Immutable:** Orion refuses requests to modify constitutional docs
- **Recovery:** If user loses private key, they cannot re-sign after reinstall

## First Run Security Flow

1. User clicks "Start" in boot sequence
2. Orion generates Ed25519 keypair
3. Constitutional documents are signed with the private key
4. **CRITICAL:** Private key is displayed with security warnings
5. User must acknowledge they've saved the key before proceeding
6. Private key is cleared from memory (never stored)
7. Only the public key remains for future verification

## Local LLM URL (SSRF Mitigation)

- **Validation:** The local LLM base URL is validated whenever it is set (UI, birth flow, or from `LOCAL_LLM_BASE_URL` env) and when loaded from config. Only **http** or **https** URLs are allowed. The host must be **localhost**, **127.0.0.1**, or **::1**. Private IP ranges (e.g. 169.254.x.x, 10.x, 192.168.x) and other hosts are rejected to prevent SSRF (e.g. cloud metadata, internal services).
- **Defense in depth:** The HTTP provider re-validates the URL before each heartbeat and completion request. If config was tampered with, the first request will fail.

## Dependency and CI Security

- **CI and Dependabot:** See CONTRIBUTING.md and the repository for current status. When CI is enabled, run `cargo audit` and `npm audit` as part of your workflow; document any exceptions for known advisories.

## Frontend Security

- The web frontend is a Vite/React SPA. Ensure no secrets or sensitive data are exposed in client bundles. Use `example.env` for variable names and keep real values in `.env` (gitignored).

## Path Validation

- **Backup:** The SQLite backup destination path is validated before write. Allowed bases: the app data directory, and (on Windows) `%USERPROFILE%\Documents`, (on Unix) `$HOME/Documents` and `$HOME`. Path traversal (e.g. `..`) is rejected; the resolved parent of the destination must be under one of these bases. Prefer using the native Save dialog (Data Archives → Backup), which lets the user pick a path; the backend re-validates before copying.

## Skill Sandbox

- **Network:** The executor checks the sandbox for network permission before running a tool that declares a network permission (domain allowlist). Other resource access (file, memory) uses the same sandbox logic but must be invoked by the code path that performs the I/O (e.g. a capability layer). Skill code that performs raw file or network I/O should go through a layer that calls the sandbox.
- **Resource limits:** Timeouts and concurrency are enforced at runtime: each tool call is bounded by `ResourceLimits::max_cpu_ms` (default 30s), and global concurrency by `max_concurrency` (default 10). Memory and storage caps are intended for capability layers and/or a future WASM runtime (see `crates/orion-skills`).

## Skill Packaging and Approval

- **Approval gating:** If `approved_skill_ids` is non-empty in config, only skills in that list may execute tools. Install and approve flows update this list and persist it to `config.json`.
- **Audit log:** Install, uninstall, and approve actions are appended to `{data_dir}/skill_audit.log` with timestamp and detail (e.g. `skill_id=...`) for traceability.
- **Signing (path):** Config supports `trusted_skill_signers` for a future signed-package format. Currently, install copies a directory with a valid `skill.toml` into `{data_dir}/skills/<id>/`; signature verification of packages is not yet implemented.

## MCP Trust

- **Server definitions:** MCP servers are configured in `AppConfig.mcp_servers` (id, name, transport, command or URL, env). Only explicitly configured servers are used.
- **Trust policy:** `mcp_trust_policy` (e.g. `allow_list_only`, `allowed_http_hosts`) restricts which HTTP hosts are allowed for stdio/HTTP MCP. Use allowlists to avoid data exfiltration to untrusted hosts.
- **Tool confirmation:** Tools that declare `requires_confirmation` should be gated in the UI before invocation; the backend does not enforce confirmation (UI responsibility).

## Pro Sidecar Security

- **API key forwarding:** When Pro tier is active, the system sends API keys to the sidecar (`PRO_MODE_SIDECAR_URL`) for parallel provider comparison. The sidecar should run on localhost or a trusted network only.
- **No key persistence:** The sidecar receives keys per-request and does not persist them.
- **Network scope:** `PRO_MODE_SIDECAR_URL` should point to a local or trusted service. Do not expose the sidecar to untrusted networks.

## Quick-Start Birth Security

- **Automatic signing:** Quick-start birth generates a keypair, signs documents, and stores the public key. The private key is **not** returned to the caller and is discarded after signing. This means quick-start agents cannot have their documents re-signed externally.
- **Standard documents:** Quick-start uses default constitutional templates rather than mentor-customized ones.

## Threat Model Summary

| Threat | Mitigation |
|--------|------------|
| Tampered constitutional docs | Signature verification at boot |
| Lost private key | Clear warnings during setup; user responsibility |
| Compromised private key | User can detect via failed verification |
| Man-in-the-middle on download | Installer signatures (future: code signing) |
| Local privilege escalation | DPAPI uses user scope, not machine scope |
| Skill supply-chain abuse | Approval list; audit log; (future) signed packages + trusted signers |
| MCP server exfiltration | Per-server config; HTTP allowlist in trust policy |
| UI sandbox escape (MCP Apps) | Sandboxed iframe + CSP; no elevated privileges to host |
| Pro sidecar key exposure | Keys sent per-request only; sidecar must be localhost/trusted |
