# Skill Build & Configuration Troubleshooting Playbook (for Orion Entities)

## Purpose
When configuring or building a new skill (native or MCP) fails, use this playbook to:
- Stop guessing.
- Bisect the problem quickly (discard ~50% of hypotheses per step).
- Produce a clear next action with verification.

This is a deterministic troubleshooting protocol, not "try random things until it works."

---

## Golden Rules (non-negotiable)
1. **Never retry the same failing tool call with the same arguments.**
   - If you retry, you must change *one* variable (args, endpoint, domain, timeout, environment, etc.).
2. **Always identify the failure stage before changing anything.**
3. **Change one thing at a time; verify after each change.**
4. **Prefer minimal reproduction over full integration testing.**
   - Get a "hello world" success path first, then add complexity.

---

## The Skill Failure Pipeline (where bugs live)
Almost all failures are one of these stages:

A) **Discovery/Registration**
- Skill/tool doesn't appear in the skill list.

B) **Readiness/Secrets**
- Skill appears but shows "not configured", missing keys, or MissingSecret-type errors.

C) **Safety Gate**
- Tool invocation is blocked by policy/safety gate (explicit "blocked" response).

D) **Permissions/Sandbox**
- Permission denied (commonly: network domain not granted).

E) **Runtime/Connectivity/Dependencies**
- DNS, TLS, HTTP errors, server offline, toolbox unreachable, missing binaries.

F) **Skill Logic/Output**
- Skill runs but returns wrong format, partial output, parsing failures, etc.

Your job is to *locate the stage* first, then fix.

---

## 90-Second Triage (do this before "fixing")
Capture these facts in a scratchpad "Failure Record":

- skill_id / skill_name
- tool_name
- exact arguments (redact secrets)
- exact error text
- structured_failure (if present)
- trust tier (Verified / AgentBuilt / Untrusted)
- whether confirmation was required
- timestamp

Then run the 3 quick checks:

### Quick Check 1 — Is the skill/tool registered?
Call: `GET /api/agents/{id}/skills`
- If the skill/tool is missing: you are in Stage A (Registration).
- If present: continue.

### Quick Check 2 — Is it ready or missing secrets?
Call: `GET /api/agents/{id}/skills/missing-secrets`
- If your skill is listed there: you are in Stage B (Secrets/Readiness).

### Quick Check 3 — Can you run the tool directly?
Call: `POST /api/agents/{id}/skills/{skill_id}/execute`
Body example:
```json
{
  "tool": "tool_name_here",
  "params": { ... },
  "confirm": false
}
```

If you receive `confirmation_required` with a nonce:
- Re-run the same endpoint with `"confirm": true` and the returned `"nonce"`.

---

## Binary Split Protocol (the "half split" method)
After triage, choose the branch that eliminates the most hypotheses.

### Branch 1: Tool missing => Stage A (Registration)
**Hypotheses eliminated:** secrets, permissions, runtime, logic.

Actions:
- If it's an MCP skill: confirm it was registered and the server is reachable.
- If it's a native skill: confirm it is compiled and explicitly registered in the app's skill registry.

Verification:
- Re-run `GET /api/agents/{id}/skills` and confirm the tool appears.

---

### Branch 2: Missing secrets / "not configured" => Stage B (Secrets)
**Hypotheses eliminated:** permissions, runtime, logic (mostly).

Actions:
- Identify missing secret names.
- Store the needed secret(s) using the appropriate vault/secret tool.
- Do not store random keys under arbitrary names; use the exact secret name declared by the skill.

Verification:
- `GET /api/agents/{id}/skills/missing-secrets` no longer lists it.
- Re-run tool execution.

---

### Branch 3: "Blocked by safety policy" => Stage C (Safety Gate)
**Hypotheses eliminated:** registration, secrets, network connectivity, code bugs.

Actions:
- Change approach to a safe alternative:
  - Use a different tool.
  - Narrow scope of the request.
  - Remove disallowed content or target.
- Do NOT brute-force or attempt to bypass safety.

Verification:
- Re-run with the new approach and confirm the block no longer triggers.

---

### Branch 4: Permission denied => Stage D (Permissions/Sandbox)
Common case: tool requires network access but the manifest doesn't grant it.

Actions:
- If the tool needs network:
  - Ensure the skill's manifest grants the needed network scope (domain allowlist or full).
  - Ensure the tool's required permissions match what the manifest grants.
- If the skill is AgentBuilt:
  - Assume high-risk permissions may be filtered by trust tier.
  - Redesign to avoid those permissions OR escalate for Verified approval.

Verification:
- Re-run the tool; it should pass permission checks and proceed to runtime.

---

### Branch 5: Connection errors / server offline / toolbox unreachable => Stage E (Runtime)
Actions:
- Validate the target is reachable *from the same runtime context*.
  - "Works on my host" is irrelevant if the agent runs in a container/network namespace.
- If using toolbox:
  - Run toolbox status checks.
  - Confirm toolbox URL/secret env vars are correct.
- If HTTP:
  - Try a minimal GET to a known endpoint.
  - Check DNS resolution, TLS errors, and timeouts.

Verification:
- A minimal request succeeds (health endpoint, ping-equivalent, or simple GET).

---

### Branch 6: Runs but wrong output => Stage F (Logic/Output)
Actions:
- Reduce input to the smallest case that still fails.
- Add explicit validation at tool boundary (parameter schema, required fields).
- If output is too large or slow:
  - Add paging/chunking.
  - Return a summary + a cursor/next_token.

Verification:
- A minimal test case passes and output matches the declared schema.

---

## MCP Skill Build Troubleshooting (common)
If an MCP skill won't show up or won't initialize:

1) Confirm the MCP server is **running** and reachable at the configured base URL.
2) Confirm it can respond to a health/list-tools request.
3) Re-register the MCP skill (or restart the agentic run that loads persisted MCP servers).
4) If the server is intermittently offline, treat it as an availability problem and add retries/backoff on the server side.

---

## Native Skill Build Troubleshooting (common)
If a native skill compiles but doesn't appear:
- It probably wasn't registered into the running registry.
- Ensure the app registers it at startup (or in the relevant init path).

If a native skill appears but fails at runtime:
- Check secrets, then permissions, then actual runtime dependencies.

---

## Orion-Specific Gotchas

### Network permission mismatches look like "mysterious PermissionDenied"
Orion's SkillExecutor builds a network audit action based on `tool.required_permissions` and denies execution if the sandbox doesn't grant it. If a tool declares `Network(Full)` but the skill manifest doesn't grant network permission, the fix is manifest/tool alignment (Stage D), not "retry the request."

### Trust tier changes timeouts + strips permissions
Resource limits and permission filtering differ by trust tier:
- **Verified**: ~30s timeout, full permission set
- **AgentBuilt**: ~15s timeout, ShellExecute stripped
- **Untrusted**: ~10s timeout, most dangerous permissions stripped

A "works for Verified skill, times out for AgentBuilt" failure is often solved by chunking the work or promoting the skill tier (with mentor approval), not by micro-optimizing code.

### MCP skills persist but can fail to load if the server is offline
Persisted MCP server definitions are loaded and registered at agentic task launch; initialization can fail if the MCP server isn't reachable at load time.

### Prefer structured_failure over string errors
New skills should return `structured_failure` whenever they fail (instead of only a string). Orion's tool output type is explicitly designed so the governor can recover from structured failures without error-string parsing.

---

## When to Consult Mentor (escalate)
Escalate when:
- You need elevated permissions that are filtered by trust tier.
- A change is irreversible/destructive (deletes, system installs on host).
- A secret/credential is required and you don't have it.
- You can't validate reachability (network policy / firewall / host config).

When escalating, provide your Failure Record + the branch you're in + your proposed fix.

---

## Template: Failure Record (copy/paste)
```
- Stage (A/B/C/D/E/F):
- skill_id:
- tool_name:
- args (redacted):
- error:
- structured_failure:
- trust tier:
- confirmation required?:
- last verification performed:
- next action:
```
