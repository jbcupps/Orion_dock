const getBaseUrl = (): string => {
  if (import.meta.env.VITE_API_URL) return import.meta.env.VITE_API_URL;
  return '';
};

export interface HealthResponse {
  status: string;
}

export interface StatusResponse {
  memory_backend: string;
  local_llm_configured: boolean;
  birth_model: string | null;
  birth_complete: boolean;
  birth_stage: string | null;
  agent_name?: string | null;
}

export interface AgentIdentityInfo {
  id: string;
  name: string;
  directory: string;
  birth_complete: boolean;
  birth_date: string | null;
}

export async function fetchHealth(): Promise<HealthResponse> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/health`);
  if (!res.ok) throw new Error(`Health check failed: ${res.status}`);
  return res.json();
}

export async function fetchStatus(): Promise<StatusResponse> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/status`);
  if (!res.ok) throw new Error(`Status failed: ${res.status}`);
  return res.json();
}

export async function fetchIdentities(): Promise<AgentIdentityInfo[]> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/identities`);
  if (!res.ok) throw new Error(`Identities failed: ${res.status}`);
  return res.json();
}

export async function createAgent(name: string): Promise<{ id: string }> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/agents`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  });
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Create failed: ${res.status}`);
  }
  return res.json();
}

export async function loadAgent(id: string): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/agents/${encodeURIComponent(id)}/load`, {
    method: 'POST',
  });
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Load failed: ${res.status}`);
  }
}

export interface BirthStateResponse {
  stage: string;
  private_key_base64?: string | null;
}

export async function fetchBirthState(
  agentId: string
): Promise<BirthStateResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/birth/state`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Birth state failed: ${res.status}`);
  }
  return res.json();
}

export async function advanceDarkness(agentId: string): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/birth/advance-darkness`,
    { method: 'POST' }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Advance darkness failed: ${res.status}`);
  }
}

export async function setIgnition(
  agentId: string,
  local_llm_base_url?: string
): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/birth/ignition`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        local_llm_base_url: local_llm_base_url || undefined,
      }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Ignition failed: ${res.status}`);
  }
}

export interface GenesisPathItem {
  id: string;
  label: string;
  description: string;
  estimated_time: string;
  depth?: string;
}

export async function fetchGenesisPaths(): Promise<GenesisPathItem[]> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/genesis/paths`);
  if (!res.ok) throw new Error(`Genesis paths failed: ${res.status}`);
  return res.json();
}

export async function startGenesis(
  agentId: string,
  path: string,
  depth?: string
): Promise<{ ok: boolean; path: string; state?: string; prompt?: string; choices?: string[] }> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/agents/${encodeURIComponent(agentId)}/genesis/start`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ path, depth }),
  });
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Genesis start failed: ${res.status}`);
  }
  return res.json();
}

export async function forgeSelect(
  agentId: string,
  choice: number
): Promise<{
  state: string;
  prompt?: string;
  choices?: string[];
  archetype?: string;
  soul_hash?: string;
  sigil_art?: string;
  weights?: Record<string, number>;
}> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/genesis/forge/select`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ choice }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Forge select failed: ${res.status}`);
  }
  return res.json();
}

export async function fetchGenesisState(
  agentId: string
): Promise<{ path: string | null; depth?: string | null }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/genesis/state`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Genesis state failed: ${res.status}`);
  }
  return res.json();
}

export async function fetchForgeState(
  agentId: string
): Promise<{
  active: boolean;
  state?: string;
  prompt?: string;
  choices?: string[];
  archetype?: string;
  soul_hash?: string;
  sigil_art?: string;
  weights?: Record<string, number>;
}> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/genesis/forge/state`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Forge state failed: ${res.status}`);
  }
  return res.json();
}

export interface ForgeCrystallizeBody {
  name: string;
  purpose?: string;
  personality?: string;
}

export async function forgeCrystallize(
  agentId: string,
  body: ForgeCrystallizeBody
): Promise<{ ok: boolean }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/genesis/forge/crystallize`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: body.name.trim(),
        purpose: body.purpose?.trim() || undefined,
        personality: body.personality?.trim() || undefined,
      }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Forge crystallize failed: ${res.status}`);
  }
  return res.json();
}

export async function completeEmergence(agentId: string): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/birth/complete-emergence`,
    { method: 'POST' }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Complete emergence failed: ${res.status}`);
  }
}

export interface BirthChatMessageItem {
  role: string;
  content: string;
}

export async function fetchBirthChatHistory(
  agentId: string
): Promise<{ messages: BirthChatMessageItem[] }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/birth/chat/history`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Birth chat history failed: ${res.status}`);
  }
  return res.json();
}

export async function sendBirthChat(
  agentId: string,
  message: string
): Promise<{
  assistant_content: string;
  tool_requests: { name: string; arguments: unknown }[];
  crystallized?: boolean;
}> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/birth/chat`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: message.trim() }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Birth chat failed: ${res.status}`);
  }
  return res.json();
}

// ---- Connectivity API ----

export interface ConnectivityChatResponse {
  assistant_content: string;
  tool_requests: { name: string; arguments: unknown }[];
  stored_providers: string[];
  key_stored?: { provider: string; validated: boolean };
}

export async function sendConnectivityChat(
  agentId: string,
  message: string
): Promise<ConnectivityChatResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/connectivity/chat`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: message.trim() }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Connectivity chat failed: ${res.status}`);
  }
  return res.json();
}

export async function fetchConnectivityChatHistory(
  agentId: string
): Promise<{ messages: BirthChatMessageItem[] }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/connectivity/chat/history`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Connectivity chat history failed: ${res.status}`);
  }
  return res.json();
}

export interface StoreKeyResponse {
  ok: boolean;
  provider: string;
  validated: boolean;
}

export async function storeProviderKey(
  agentId: string,
  provider: string,
  key: string,
  validate = true
): Promise<StoreKeyResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/connectivity/keys`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ provider, key, validate }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Store key failed: ${res.status}`);
  }
  return res.json();
}

export async function fetchStoredProviders(
  agentId: string
): Promise<{ providers: string[] }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/connectivity/providers`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Providers failed: ${res.status}`);
  }
  return res.json();
}

// ---- Operational Chat API (post-birth) ----

export async function fetchChatHistory(
  agentId: string
): Promise<{ messages: BirthChatMessageItem[] }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/chat/history`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Chat history failed: ${res.status}`);
  }
  return res.json();
}

export interface OperationalChatResponse {
  assistant_content: string;
  tool_executed?: { name: string; provider: string };
  stored_providers?: string[];
}

export async function sendChat(
  agentId: string,
  message: string
): Promise<OperationalChatResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/chat`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: message.trim() }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Chat failed: ${res.status}`);
  }
  return res.json();
}

// ---- External Verification API ----

export interface AgentIdentityBundle {
  agent_id: string;
  name: string | null;
  pubkey_base64: string;
  birth_complete: boolean;
  birth_date: string | null;
}

export async function fetchAgentIdentity(
  agentId: string
): Promise<AgentIdentityBundle> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/identity`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Agent identity failed: ${res.status}`);
  }
  return res.json();
}

export interface ConstitutionDocument {
  name: string;
  tier: string;
  content: string;
  signature: string;
  signed_at: string;
}

export interface ConstitutionResponse {
  agent_id: string;
  pubkey_base64: string;
  documents: ConstitutionDocument[];
}

export async function fetchConstitution(
  agentId: string
): Promise<ConstitutionResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/constitution`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Constitution failed: ${res.status}`);
  }
  return res.json();
}

export interface DocumentVerifyResult {
  name: string;
  valid: boolean;
  error?: string;
}

export interface VerifyResponse {
  agent_id: string;
  all_valid: boolean;
  results: DocumentVerifyResult[];
}

export async function verifyAgent(
  agentId: string
): Promise<VerifyResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/verify`,
    { method: 'POST' }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Verify failed: ${res.status}`);
  }
  return res.json();
}
