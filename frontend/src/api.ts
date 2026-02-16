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
  birth_model_ready?: boolean | null;
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

export async function createAgent(
  name: string,
  quickStart = false
): Promise<{ id: string }> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/agents`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, quick_start: quickStart }),
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
): Promise<{ ok: boolean; path: string; completed?: boolean; state?: string; prompt?: string; choices?: string[] }> {
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

export async function removeProviderKey(
  agentId: string,
  provider: string
): Promise<{ ok: boolean }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/connectivity/keys/${encodeURIComponent(provider)}`,
    { method: 'DELETE' }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Remove key failed: ${res.status}`);
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

export interface TierModels {
  fast: string;
  standard: string;
  pro: string;
}

export interface ProviderCatalogEntry {
  available_models: string[];
  source: string;
  last_refreshed?: string | null;
  warnings: string[];
  validated: boolean;
}

export interface TierModelsResponse {
  active_provider?: string | null;
  models: Record<string, TierModels>;
  catalog?: Record<string, ProviderCatalogEntry>;
}

export interface ModelValidationResult {
  model: string;
  valid: boolean;
  error?: string | null;
}

export interface ProviderModelValidation {
  fast: ModelValidationResult;
  standard: ModelValidationResult;
  pro: ModelValidationResult;
}

export interface ValidateModelsResponse {
  results: Record<string, ProviderModelValidation>;
}

export async function fetchTierModels(
  agentId: string
): Promise<TierModelsResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/tier-models`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Tier models failed: ${res.status}`);
  }
  return res.json();
}

export async function updateTierModels(
  agentId: string,
  models: Record<string, TierModels>
): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/tier-models`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ models }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Update tier models failed: ${res.status}`);
  }
}

export async function refreshProviderCatalog(
  agentId: string,
  provider?: string
): Promise<{ ok: boolean; refreshed: string[] }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/tier-models/refresh`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ provider: provider ?? null }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Refresh catalog failed: ${res.status}`);
  }
  return res.json();
}

export async function validateTierModels(
  agentId: string,
  provider?: string
): Promise<ValidateModelsResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/tier-models/validate`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ provider: provider ?? null }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Validate models failed: ${res.status}`);
  }
  return res.json();
}

export async function resetTierModels(
  agentId: string,
  provider?: string
): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/tier-models/reset`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ provider: provider ?? null }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Reset tier models failed: ${res.status}`);
  }
}

export async function setActiveProvider(
  agentId: string,
  provider: string
): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/active-provider`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ provider }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Set active provider failed: ${res.status}`);
  }
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
  attachment_notice?: string;
  tool_log?: {
    tool_name: string;
    skill_name?: string;
    success: boolean;
    output: string;
  }[];
}

export interface ChatAttachmentUploadItem {
  attachment_id: string;
  file_name: string;
  size_bytes: number;
  detected_mime: string;
  detected_kind: string;
  parse_status: string;
  warning_count: number;
  summary: string;
}

export interface ChatAttachmentUploadResponse {
  attachments: ChatAttachmentUploadItem[];
}

export async function uploadChatAttachments(
  agentId: string,
  files: File[]
): Promise<ChatAttachmentUploadResponse> {
  if (!files.length) {
    return { attachments: [] };
  }
  const base = getBaseUrl();
  const formData = new FormData();
  for (const file of files) {
    formData.append('files', file, file.name);
  }
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/chat/attachments`,
    {
      method: 'POST',
      body: formData,
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Attachment upload failed: ${res.status}`);
  }
  return res.json();
}

export async function sendChat(
  agentId: string,
  message: string,
  routerMode?: 'auto' | 'think_hard' | 'think_harder',
  attachmentIds: string[] = []
): Promise<OperationalChatResponse> {
  const base = getBaseUrl();
  const payload: Record<string, unknown> = { message: message.trim() };
  if (routerMode) {
    payload.router_mode = routerMode;
  }
  if (attachmentIds.length > 0) {
    payload.attachment_ids = attachmentIds;
  }
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/chat`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Chat failed: ${res.status}`);
  }
  return res.json();
}

// ---- Agentic Runs (Jobs) API ----

export interface AgenticRunInfo {
  task_id: string;
  goal: string;
  status: string;
  turns: number;
  tool_calls: number;
  source?: string;
  summary?: string;
  started_at: string;
  completed_at?: string;
}

export async function fetchAgenticRuns(
  agentId: string
): Promise<AgenticRunInfo[]> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/agent/runs`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Fetch agentic runs failed: ${res.status}`);
  }
  return res.json();
}

// ---- Orchestration Jobs API ----

export type OrchestrationJobMode = 'id_check' | 'agentic_run';
export type SignificanceLevel = 'low' | 'medium' | 'high';
export type OrchestrationDecision = 'silent_log' | 'spawn_agentic' | 'flag_mentor';

export interface SignificancePolicy {
  medium_keywords?: string[];
  high_keywords?: string[];
  escalate_medium?: boolean;
  flag_high_to_mentor?: boolean;
}

export interface OrchestrationJob {
  job_id: string;
  name: string;
  cron: string;
  enabled: boolean;
  mode: OrchestrationJobMode;
  goal_template: string;
  significance_policy: SignificancePolicy;
  created_at: string;
  updated_at: string;
  last_run_at?: string;
  next_run_at?: string;
  last_status?: string;
  last_significance?: SignificanceLevel;
}

export interface OrchestrationJobLogEntry {
  entry_id: string;
  job_id: string;
  job_name: string;
  mode: OrchestrationJobMode;
  started_at: string;
  completed_at: string;
  significance: SignificanceLevel;
  decision: OrchestrationDecision;
  status: string;
  summary: string;
  task_id?: string;
}

export interface JobRunNowResponse {
  job_id: string;
  status: string;
  decision: OrchestrationDecision;
  significance: SignificanceLevel;
  task_id?: string;
  summary: string;
}

export async function fetchOrchestrationJobs(
  agentId: string
): Promise<{ jobs: OrchestrationJob[] }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/orchestration/jobs`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Fetch orchestration jobs failed: ${res.status}`);
  }
  return res.json();
}

export async function createOrchestrationJob(
  agentId: string,
  payload: {
    name: string;
    cron: string;
    mode: OrchestrationJobMode;
    goal_template: string;
    enabled?: boolean;
    significance_policy?: SignificancePolicy;
  }
): Promise<OrchestrationJob> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/orchestration/jobs`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Create orchestration job failed: ${res.status}`);
  }
  return res.json();
}

export async function updateOrchestrationJob(
  agentId: string,
  jobId: string,
  payload: {
    name?: string;
    cron?: string;
    mode?: OrchestrationJobMode;
    goal_template?: string;
    enabled?: boolean;
    significance_policy?: SignificancePolicy;
  }
): Promise<OrchestrationJob> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/orchestration/jobs/${encodeURIComponent(jobId)}`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Update orchestration job failed: ${res.status}`);
  }
  return res.json();
}

export async function setOrchestrationJobEnabled(
  agentId: string,
  jobId: string,
  enabled: boolean
): Promise<OrchestrationJob> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/orchestration/jobs/${encodeURIComponent(jobId)}/enable`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ enabled }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Enable orchestration job failed: ${res.status}`);
  }
  return res.json();
}

export async function deleteOrchestrationJob(
  agentId: string,
  jobId: string
): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/orchestration/jobs/${encodeURIComponent(jobId)}/delete`,
    { method: 'POST' }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Delete orchestration job failed: ${res.status}`);
  }
}

export async function runOrchestrationJobNow(
  agentId: string,
  jobId: string
): Promise<JobRunNowResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/orchestration/jobs/${encodeURIComponent(jobId)}/run`,
    { method: 'POST' }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Run orchestration job failed: ${res.status}`);
  }
  return res.json();
}

export async function fetchOrchestrationLogs(
  agentId: string,
  limit = 25
): Promise<{ logs: OrchestrationJobLogEntry[] }> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/orchestration/logs?limit=${encodeURIComponent(String(limit))}`
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Fetch orchestration logs failed: ${res.status}`);
  }
  return res.json();
}

// ---- Agentic Loop API ----

export interface AgenticRunRequest {
  goal: string;
  max_turns?: number;
  auto_approve_safe_tools?: boolean;
  router_mode?: 'auto' | 'think_hard' | 'think_harder';
}

export interface AgenticRunStartResponse {
  task_id: string;
  stream_url: string;
}

export interface AgenticEventThinking {
  event: 'Thinking';
  data: { turn: number; content: string };
}

export interface AgenticEventToolCall {
  event: 'ToolCall';
  data: { turn: number; tool_name: string; arguments: unknown };
}

export interface AgenticEventToolResult {
  event: 'ToolResult';
  data: { turn: number; tool_name: string; success: boolean; output: string };
}

export interface AgenticEventMentorNeeded {
  event: 'MentorNeeded';
  data: { turn: number; question: string };
}

export interface AgenticEventConfirmationNeeded {
  event: 'ConfirmationNeeded';
  data: { turn: number; tool_name: string; arguments: unknown };
}

export interface AgenticEventDone {
  event: 'Done';
  data: { summary: string; status: string; turns_used: number; tool_calls: number };
}

export interface AgenticEventError {
  event: 'Error';
  data: { message: string };
}

export type AgenticEvent =
  | AgenticEventThinking
  | AgenticEventToolCall
  | AgenticEventToolResult
  | AgenticEventMentorNeeded
  | AgenticEventConfirmationNeeded
  | AgenticEventDone
  | AgenticEventError;

export async function startAgenticRun(
  agentId: string,
  request: AgenticRunRequest
): Promise<AgenticRunStartResponse> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/agent/run`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Start agentic run failed: ${res.status}`);
  }
  return res.json();
}

export function subscribeToAgenticStream(
  agentId: string,
  taskId: string,
  onEvent: (eventName: string, data: unknown) => void,
  onError?: (error: Event) => void
): EventSource {
  const base = getBaseUrl();
  const url = `${base}/api/agents/${encodeURIComponent(agentId)}/agent/stream?task=${encodeURIComponent(taskId)}`;
  const es = new EventSource(url);

  const eventTypes = [
    'thinking',
    'tool_call',
    'tool_result',
    'mentor_needed',
    'confirmation_needed',
    'done',
    'error',
    'lagged',
  ];

  for (const eventType of eventTypes) {
    es.addEventListener(eventType, (event: MessageEvent) => {
      try {
        const data = JSON.parse(event.data);
        onEvent(eventType, data);
      } catch {
        onEvent(eventType, event.data);
      }
    });
  }

  if (onError) {
    es.onerror = onError;
  }

  return es;
}

export async function respondToAgent(
  agentId: string,
  taskId: string,
  response: string
): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/agent/respond`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_id: taskId, response }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Respond to agent failed: ${res.status}`);
  }
}

export async function confirmAgentTool(
  agentId: string,
  taskId: string,
  approved: boolean
): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/agent/confirm`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_id: taskId, approved }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Confirm tool failed: ${res.status}`);
  }
}

export async function cancelAgenticRun(
  agentId: string,
  taskId: string
): Promise<void> {
  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/agent/cancel`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_id: taskId }),
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Cancel agentic run failed: ${res.status}`);
  }
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

// ---- Agent Export API ----

export async function exportAgent(
  agentId: string,
  privateKeyBase64: string,
  agentName?: string
): Promise<void> {
  const trimmedKey = privateKeyBase64.trim();
  if (!trimmedKey) {
    throw new Error('Private key is required for export');
  }

  const base = getBaseUrl();
  const res = await fetch(
    `${base}/api/agents/${encodeURIComponent(agentId)}/export`,
    {
      headers: {
        'x-orion-private-key': trimmedKey,
      },
    }
  );
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Export failed: ${res.status}`);
  }
  const blob = await res.blob();
  const safeName = (agentName || 'agent').replace(/[^a-zA-Z0-9_-]/g, '-');
  const date = new Date().toISOString().slice(0, 10).replace(/-/g, '');
  const filename = `orion-agent-${safeName}-${date}.json`;
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

// ---- Agent Import API ----

export interface ImportAgentResponse {
  id: string;
  name: string;
}

export async function importAgent(
  exportData: unknown,
  privateKeyBase64: string
): Promise<ImportAgentResponse> {
  const trimmedKey = privateKeyBase64.trim();
  if (!trimmedKey) {
    throw new Error('Private key is required for import');
  }

  const base = getBaseUrl();
  const res = await fetch(`${base}/api/agents/import`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      export: exportData,
      private_key_base64: trimmedKey,
    }),
  });
  if (!res.ok) {
    const err = await res.text();
    throw new Error(err || `Import failed: ${res.status}`);
  }
  return res.json();
}
