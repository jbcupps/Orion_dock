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
