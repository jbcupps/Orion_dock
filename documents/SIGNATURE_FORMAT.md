# Orion Dock Signature Format Specification

Version: 1.0
Date: 2026-02-11

## Overview

Orion Dock uses Ed25519 digital signatures to ensure the integrity and authenticity of constitutional documents. Each agent has a unique cryptographic identity generated during the Darkness stage of its birth process. This document specifies the exact formats used so that external systems (SAO, Ethical_AI_Reg, third-party verifiers) can independently verify agent identity.

---

## Ed25519 Key Format

- **Algorithm:** Ed25519 (RFC 8032)
- **Signing key:** 32-byte seed (expanded internally by Ed25519)
- **Public key:** 32-byte compressed Edwards point
- **Signature:** 64-byte Ed25519 signature

### Public Key File

- **Path:** `{agent_dir}/external_pubkey.bin`
- **Format:** Raw 32-byte binary (no headers, no encoding)
- **Encoding for API/transport:** Standard base64 (RFC 4648, no padding variants accepted but standard padding produced)

### Private Key

- **Shown once** during Darkness stage as standard base64-encoded 32-byte seed
- **Never persisted** in plaintext (the signing key file used during birth is encrypted via DPAPI on Windows or plaintext with dev warning on Linux)
- **Deleted** after Emergence completes

---

## Signable Bytes Format

The bytes that are signed for each document follow this exact format:

```
{doc_name}|{tier}|{content}
```

- **doc_name:** The document filename (e.g., `soul.md`, `ethics.md`, `instincts.md`)
- **tier:** Stable lowercase string from `DocumentTier::as_str()`:
  - `"constitutional"` — immutable after signing
  - `"mentor_editable"` — can be re-signed by mentor
- **content:** The full UTF-8 content of the document file
- **Separator:** ASCII pipe character `|` (0x7C)
- **Encoding:** The entire string is encoded as UTF-8 bytes before signing

### Example

For a document `soul.md` with content `I am Orion.`:

```
soul.md|constitutional|I am Orion.
```

The UTF-8 byte representation of this string is what gets signed.

---

## Signature File Format (.sig)

Each constitutional document `{name}` has a corresponding `{name}.sig` file containing JSON:

```json
{
  "signature": "<base64-encoded 64-byte Ed25519 signature>",
  "tier": "Constitutional",
  "signed_at": "2026-02-11T12:00:00.000000Z"
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `signature` | string | Standard base64-encoded 64-byte Ed25519 signature |
| `tier` | string | Serde-serialized `DocumentTier` enum: `"Constitutional"` or `"MentorEditable"` |
| `signed_at` | string | ISO 8601 / RFC 3339 timestamp (UTC) |

**Note:** The `tier` field in the `.sig` JSON uses the Serde-serialized enum name (`"Constitutional"`, `"MentorEditable"`), while the signable bytes use the stable lowercase form (`"constitutional"`, `"mentor_editable"`). Verifiers must map accordingly.

---

## Constitutional Documents

The following documents are signed during the Emergence stage:

| Document | Tier | Purpose |
|----------|------|---------|
| `soul.md` | constitutional | Agent identity, nature, and relationship to mentor |
| `ethics.md` | constitutional | Triangle Ethic (Deontology, Virtue, Teleology) |
| `instincts.md` | constitutional | Pre-cognitive behaviors (Privacy Prime, Sentry Mode, etc.) |

All three live in `{agent_dir}/docs/`.

---

## Verification Algorithm

To verify an agent's constitutional documents:

1. **Load the public key** from `{agent_dir}/external_pubkey.bin` (32 raw bytes)
2. For each document (`soul.md`, `ethics.md`, `instincts.md`):
   a. Read the document content from `{agent_dir}/docs/{name}`
   b. Read and parse `{agent_dir}/docs/{name}.sig` as JSON
   c. Map the `tier` field to the signable tier string:
      - `"Constitutional"` -> `"constitutional"`
      - `"MentorEditable"` -> `"mentor_editable"`
   d. Construct the signable string: `"{name}|{tier_str}|{content}"`
   e. Encode the signable string as UTF-8 bytes
   f. Decode the `signature` field from base64 to get 64 raw bytes
   g. Verify the Ed25519 signature against the public key and signable bytes
3. All three documents must pass for the agent's identity to be considered valid

---

## API Endpoints

### `GET /api/agents/:id/identity`

Returns the agent's public key and birth status.

**Response:**
```json
{
  "agent_id": "uuid",
  "name": "AgentName",
  "pubkey_base64": "<base64 32-byte public key>",
  "birth_complete": true,
  "birth_date": "2026-02-11T12:00:00Z"
}
```

### `GET /api/agents/:id/constitution`

Returns all signed constitutional documents with their content and signatures. Only available after birth is complete.

**Response:**
```json
{
  "agent_id": "uuid",
  "pubkey_base64": "<base64 32-byte public key>",
  "documents": [
    {
      "name": "soul.md",
      "tier": "constitutional",
      "content": "...",
      "signature": "<base64 64-byte signature>",
      "signed_at": "2026-02-11T12:00:00+00:00"
    }
  ]
}
```

### `POST /api/agents/:id/verify`

Server-side verification of all constitutional documents. Only available after birth is complete.

**Response:**
```json
{
  "agent_id": "uuid",
  "all_valid": true,
  "results": [
    { "name": "soul.md", "valid": true },
    { "name": "ethics.md", "valid": true },
    { "name": "instincts.md", "valid": true }
  ]
}
```

On failure, individual results include an `error` field:
```json
{ "name": "soul.md", "valid": false, "error": "Signature verification failed for soul.md" }
```

---

## External Verification Example (Python)

```python
import base64
import json
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

# 1. Load public key (raw 32 bytes)
with open("external_pubkey.bin", "rb") as f:
    pubkey = Ed25519PublicKey.from_public_bytes(f.read())

# 2. For each document
for doc_name in ["soul.md", "ethics.md", "instincts.md"]:
    with open(f"docs/{doc_name}", "r", encoding="utf-8") as f:
        content = f.read()
    with open(f"docs/{doc_name}.sig", "r") as f:
        sig_meta = json.load(f)

    # 3. Map tier
    tier_map = {"Constitutional": "constitutional", "MentorEditable": "mentor_editable"}
    tier_str = tier_map[sig_meta["tier"]]

    # 4. Construct signable bytes
    signable = f"{doc_name}|{tier_str}|{content}".encode("utf-8")

    # 5. Decode signature
    signature = base64.b64decode(sig_meta["signature"])

    # 6. Verify
    try:
        pubkey.verify(signature, signable)
        print(f"{doc_name}: VALID")
    except Exception as e:
        print(f"{doc_name}: INVALID - {e}")
```

---

## Security Considerations

- The signing key is generated once and shown to the mentor as base64. It is the mentor's responsibility to store it securely.
- The signing key file on disk (used during birth) is encrypted with DPAPI on Windows. On Linux/Docker, it is stored as plaintext with a logged warning. This is acceptable for development but should use a proper secrets manager in production.
- Constitutional documents (`constitutional` tier) are immutable once signed. Any modification will cause signature verification to fail.
- The public key file contains no metadata — it is exactly 32 bytes of raw Ed25519 public key.
