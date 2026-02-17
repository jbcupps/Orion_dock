import { useCallback, useEffect, useState } from 'react';
import {
  advanceDarkness,
  completeEmergence,
  fetchBirthState,
  fetchForgeState,
  fetchGenesisState,
  fetchHealth,
  fetchAgenticRuns,
  fetchMentorName,
  fetchOrchestrationJobs,
  fetchStatus,
  fetchStoredProviders,
  fetchTierModels,
  refreshProviderCatalog,
  resetTierModels,
  setActiveProvider,
  setIgnition,
  setMentorName as apiSetMentorName,
  updateTierModels,
  validateTierModels,
  type BirthStateResponse,
  type ProviderCatalogEntry,
  type RoutingTelemetry,
  type AgenticRunInfo,
  type OrchestrationJob,
  type ProviderModelValidation,
  type StatusResponse,
  type TierModels,
} from './api';
import { SUPPORTED_PROVIDERS } from './providers';
import { stageDisplayMessage, OPERATION_MESSAGE } from './birthStages';
import SplashScreen from './components/SplashScreen';
import HiveScreen from './components/HiveScreen';
import GenesisPathSelector from './components/GenesisPathSelector';
import ForgeScenario from './components/ForgeScenario';
import GenesisChat from './components/GenesisChat';
import CrystallizationChat from './components/CrystallizationChat';
import ApiKeyModal from './components/ApiKeyModal';
import ConnectivityPanel from './components/ConnectivityPanel';
import OperationalChat from './components/OperationalChat';
import AgenticPanel from './components/AgenticPanel';
import JobsTable from './components/JobsTable';
import OrchestrationJobsPanel from './components/OrchestrationJobsPanel';
import StatusBar from './components/StatusBar';
import type { HealthState } from './components/StatusBar';
import './App.css';

type AppState = 'splash' | 'hive' | 'dashboard';

function App() {
  const [appState, setAppState] = useState<AppState>('splash');
  const [currentAgentId, setCurrentAgentId] = useState<string | null>(null);
  const [health, setHealth] = useState<HealthState>('pending');
  const [status, setStatus] = useState<StatusResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [genesisPathStarted, setGenesisPathStarted] = useState<string | null>(null);
  const [forgeInitial, setForgeInitial] = useState<{
    state?: string;
    prompt?: string;
    choices?: string[];
  } | null>(null);
  const [birthState, setBirthState] = useState<BirthStateResponse | null>(null);
  const [birthStateLoading, setBirthStateLoading] = useState(false);
  const [birthStateError, setBirthStateError] = useState<string | null>(null);
  const [ignitionUrl, setIgnitionUrl] = useState('');
  const [connectivityDone, setConnectivityDone] = useState(false);
  const [storedProviders, setStoredProviders] = useState<string[]>([]);
  const [tierModels, setTierModels] = useState<Record<string, TierModels>>({});
  const [activeTierProvider, setActiveTierProvider] = useState<string | null>(null);
  const [tierModelsLoading, setTierModelsLoading] = useState(false);
  const [tierModelsEditOpen, setTierModelsEditOpen] = useState(false);
  const [tierModelsDraft, setTierModelsDraft] = useState<Record<string, TierModels>>({});
  const [tierModelsSaving, setTierModelsSaving] = useState(false);
  const [tierCatalog, setTierCatalog] = useState<Record<string, ProviderCatalogEntry>>({});
  const [tierValidation, setTierValidation] = useState<Record<string, ProviderModelValidation>>({});
  const [tierRefreshing, setTierRefreshing] = useState(false);
  const [tierValidating, setTierValidating] = useState(false);
  const [keyModalProvider, setKeyModalProvider] = useState<string | null>(null);
  const [addKeyPickerOpen, setAddKeyPickerOpen] = useState(false);
  const [showChat, setShowChat] = useState(false);
  const [dashboardTab, setDashboardTab] = useState<'chat' | 'agent' | 'jobs'>('chat');
  const [chatMode, setChatMode] = useState<'chat' | 'agentic'>('chat');
  const [launchedAgenticTask, setLaunchedAgenticTask] = useState<{
    taskId: string;
    goal: string;
  } | null>(null);
  const [routerMode, setRouterMode] = useState<'auto' | 'think_hard' | 'think_harder'>('auto');
  const [chatBusy, setChatBusy] = useState(false);
  const [agenticBusy, setAgenticBusy] = useState(false);
  const [mentorName, setMentorName] = useState<string | null>(null);
  const [routingTelemetry, setRoutingTelemetry] = useState<RoutingTelemetry | null>(null);
  const [runningAgentRuns, setRunningAgentRuns] = useState<AgenticRunInfo[]>([]);
  const [upcomingOrchestrationJobs, setUpcomingOrchestrationJobs] = useState<OrchestrationJob[]>([]);
  const cloudBusy = (chatBusy || agenticBusy) && storedProviders.length > 0;

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const h = await fetchHealth();
        if (!cancelled) setHealth((h.status as HealthState) || 'pending');
      } catch {
        if (!cancelled) {
          setHealth('error');
          setError('Health check failed');
        }
      }
    })();
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const res = await fetchMentorName();
        if (!cancelled) setMentorName(res.mentor_name);
      } catch {
        // Mentor name is optional; ignore errors
      }
    })();
    return () => { cancelled = true; };
  }, []);

  const handleSetMentorName = useCallback(async (name: string) => {
    try {
      const res = await apiSetMentorName(name);
      setMentorName(res.mentor_name);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to set mentor name');
    }
  }, []);

  useEffect(() => {
    if (appState !== 'dashboard' && appState !== 'hive') return;
    let cancelled = false;
    (async () => {
      try {
        const s = await fetchStatus();
        if (!cancelled) setStatus(s);
      } catch {
        if (!cancelled) setStatus(null);
      }
    })();
    const interval = setInterval(async () => {
      try {
        const s = await fetchStatus();
        if (!cancelled) setStatus(s);
      } catch {
        if (!cancelled) setStatus(null);
      }
    }, appState === 'dashboard' ? 3000 : 5000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [appState]);

  useEffect(() => {
    setRoutingTelemetry(null);
  }, [currentAgentId, chatMode]);

  // Fetch stored LLM providers for the current agent
  useEffect(() => {
    if (appState !== 'dashboard' || !currentAgentId) return;
    let cancelled = false;
    const load = async () => {
      try {
        const res = await fetchStoredProviders(currentAgentId);
        if (!cancelled) setStoredProviders(res.providers);
      } catch {
        // Provider list not critical
      }
    };
    load();
    const interval = setInterval(load, 30000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [appState, currentAgentId]);

  // Fetch and refresh per-provider Fast/Standard/Pro model mappings
  useEffect(() => {
    if (appState !== 'dashboard' || !currentAgentId) return;
    let cancelled = false;
    const load = async () => {
      if (!cancelled) setTierModelsLoading(true);
      try {
        const res = await fetchTierModels(currentAgentId);
        if (cancelled) return;
        setTierModels(res.models || {});
        setActiveTierProvider(res.active_provider ?? null);
        setTierCatalog(res.catalog || {});
        if (!tierModelsEditOpen) {
          setTierModelsDraft(res.models || {});
        }
      } catch {
        if (cancelled) return;
        setTierModels({});
        setActiveTierProvider(null);
        setTierCatalog({});
        if (!tierModelsEditOpen) {
          setTierModelsDraft({});
        }
      } finally {
        if (!cancelled) setTierModelsLoading(false);
      }
    };
    load();
    const interval = setInterval(load, 30000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [appState, currentAgentId, tierModelsEditOpen]);

  // Recover genesis path state on dashboard entry (handles refresh / reconnect)
  useEffect(() => {
    if (appState !== 'dashboard' || !currentAgentId) return;
    if (!status || status.birth_complete) return;
    if (status.birth_stage !== 'Genesis') return;
    if (genesisPathStarted) return; // already recovered or freshly set

    let cancelled = false;
    (async () => {
      try {
        const gs = await fetchGenesisState(currentAgentId);
        if (cancelled || !gs.path) return;
        setGenesisPathStarted(gs.path);
        // For Soul Forge, also try to recover the in-flight scenario state
        if (gs.path === 'soul_forge') {
          try {
            const fs = await fetchForgeState(currentAgentId);
            if (cancelled) return;
            if (fs.active && fs.state && fs.state !== 'crystallize' && fs.state !== 'done') {
              setForgeInitial({
                state: fs.state,
                prompt: fs.prompt,
                choices: fs.choices,
              });
            }
          } catch {
            // Forge session may be gone (server restart); placeholder will show
          }
        }
      } catch {
        // genesis_path.json may not exist; fall through to generic panel
      }
    })();
    return () => { cancelled = true; };
  }, [appState, currentAgentId, status?.birth_stage, status?.birth_complete, genesisPathStarted]);

  // Materialize Darkness and get one-time private key when in early birth
  const needBirthState =
    currentAgentId &&
    status &&
    !status.birth_complete &&
    (status.birth_stage == null || status.birth_stage === 'Darkness');
  useEffect(() => {
    if (!needBirthState || !currentAgentId) return;
    if (birthState != null && birthState.stage === 'Darkness') return;
    let cancelled = false;
    setBirthStateError(null);
    setBirthStateLoading(true);
    (async () => {
      try {
        const state = await fetchBirthState(currentAgentId);
        if (!cancelled) {
          setBirthState(state);
          setBirthStateError(null);
        }
      } catch (e) {
        if (!cancelled) {
          setBirthState(null);
          setBirthStateError(e instanceof Error ? e.message : 'Failed to load birth state');
        }
      } finally {
        if (!cancelled) setBirthStateLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [currentAgentId, needBirthState, birthState?.stage]);

  const handleSavedKey = useCallback(async () => {
    if (!currentAgentId) return;
    setError(null);
    try {
      await advanceDarkness(currentAgentId);
      setBirthState(null);
      const s = await fetchStatus();
      setStatus(s);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to advance');
    }
  }, [currentAgentId]);

  const handleIgnitionSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      if (!currentAgentId) return;
      setError(null);
      try {
        await setIgnition(
          currentAgentId,
          ignitionUrl.trim() || undefined
        );
        setIgnitionUrl('');
        const s = await fetchStatus();
        setStatus(s);
      } catch (e) {
        setError(e instanceof Error ? e.message : 'Failed to set Ignition');
      }
    },
    [currentAgentId, ignitionUrl]
  );

  const handleSplashComplete = () => {
    setAppState('hive');
  };

  const handleAgentSelected = (agentId: string) => {
    setCurrentAgentId(agentId);
    setAppState('dashboard');
  };

  const handleCreateAgent = (agentId: string) => {
    setCurrentAgentId(agentId);
    setAppState('dashboard');
  };

  const handleDisconnect = () => {
    setAppState('hive');
    setStatus(null);
    setCurrentAgentId(null);
    setGenesisPathStarted(null);
    setForgeInitial(null);
    setBirthState(null);
    setBirthStateError(null);
    setConnectivityDone(false);
    setStoredProviders([]);
    setTierModels({});
    setActiveTierProvider(null);
    setTierModelsLoading(false);
    setTierModelsEditOpen(false);
    setTierModelsDraft({});
    setTierModelsSaving(false);
    setTierCatalog({});
    setTierValidation({});
    setTierRefreshing(false);
    setTierValidating(false);
    setShowChat(false);
    setLaunchedAgenticTask(null);
    setDashboardTab('chat');
  };

  const handleGenesisStarted = useCallback(
    async (
      path: string,
      data?: { completed?: boolean; state?: string; prompt?: string; choices?: string[] }
    ) => {
      setError(null);
      setGenesisPathStarted(path);

      // Quick Start auto-completes birth; refresh status immediately.
      if (data?.completed) {
        try {
          const s = await fetchStatus();
          setStatus(s);
        } catch {
          // poll will retry
        }
        return;
      }

      if (path === 'soul_forge' && data?.state) {
        setForgeInitial({
          state: data.state,
          prompt: data.prompt,
          choices: data.choices,
        });
      }
      setStatus((prev) =>
        prev ? { ...prev, birth_stage: 'Genesis' } : null
      );
      try {
        const s = await fetchStatus();
        setStatus(s);
      } catch {
        // Keep optimistic Genesis stage; poll will retry
      }
    },
    []
  );

  const handleForgeComplete = useCallback(
    async (result: { crystallized?: boolean }) => {
      if (result.crystallized && currentAgentId) {
        try {
          const s = await fetchStatus();
          setStatus(s);
        } catch {
          // keep current status
        }
      }
      setForgeInitial(null);
    },
    [currentAgentId]
  );

  const [emergenceBusy, setEmergenceBusy] = useState(false);
  const handleCompleteEmergence = useCallback(async () => {
    if (!currentAgentId) return;
    setError(null);
    setEmergenceBusy(true);
    try {
      await completeEmergence(currentAgentId);
      const s = await fetchStatus();
      setStatus(s);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Finalize failed');
    } finally {
      setEmergenceBusy(false);
    }
  }, [currentAgentId]);

  const handleTierModelDraftChange = useCallback(
    (provider: string, tier: keyof TierModels, value: string) => {
      setTierModelsDraft((prev) => {
        const existing = prev[provider];
        if (!existing) return prev;
        return {
          ...prev,
          [provider]: {
            ...existing,
            [tier]: value,
          },
        };
      });
    },
    []
  );

  const handleSaveTierModels = useCallback(async () => {
    if (!currentAgentId) return;
    setError(null);
    setTierModelsSaving(true);
    try {
      await updateTierModels(currentAgentId, tierModelsDraft);
      const refreshed = await fetchTierModels(currentAgentId);
      setTierModels(refreshed.models || {});
      setActiveTierProvider(refreshed.active_provider ?? null);
      setTierModelsDraft(refreshed.models || {});
      setTierModelsEditOpen(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to update thinking models');
    } finally {
      setTierModelsSaving(false);
    }
  }, [currentAgentId, tierModelsDraft]);

  const handleCancelTierModelsEdit = useCallback(() => {
    setTierModelsDraft(tierModels);
    setTierModelsEditOpen(false);
  }, [tierModels]);

  const handleRefreshCatalog = useCallback(async () => {
    if (!currentAgentId) return;
    setTierRefreshing(true);
    try {
      await refreshProviderCatalog(currentAgentId);
      const res = await fetchTierModels(currentAgentId);
      setTierModels(res.models || {});
      setActiveTierProvider(res.active_provider ?? null);
      setTierCatalog(res.catalog || {});
      if (!tierModelsEditOpen) setTierModelsDraft(res.models || {});
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Refresh failed');
    } finally {
      setTierRefreshing(false);
    }
  }, [currentAgentId, tierModelsEditOpen]);

  const handleValidateModels = useCallback(async () => {
    if (!currentAgentId) return;
    setTierValidating(true);
    try {
      const res = await validateTierModels(currentAgentId);
      setTierValidation(res.results || {});
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Validation failed');
    } finally {
      setTierValidating(false);
    }
  }, [currentAgentId]);

  const handleResetProviderModels = useCallback(async (provider: string) => {
    if (!currentAgentId) return;
    try {
      await resetTierModels(currentAgentId, provider);
      const res = await fetchTierModels(currentAgentId);
      setTierModels(res.models || {});
      setTierCatalog(res.catalog || {});
      setTierModelsDraft(res.models || {});
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Reset failed');
    }
  }, [currentAgentId]);

  const handleSetActiveProvider = useCallback(async (provider: string) => {
    if (!currentAgentId) return;
    try {
      await setActiveProvider(currentAgentId, provider);
      const res = await fetchTierModels(currentAgentId);
      setActiveTierProvider(res.active_provider ?? null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Set active provider failed');
    }
  }, [currentAgentId]);

  const refreshProviders = useCallback(async () => {
    if (!currentAgentId) return;
    try {
      const res = await fetchStoredProviders(currentAgentId);
      setStoredProviders(res.providers);
    } catch { /* ignore */ }
  }, [currentAgentId]);

  useEffect(() => {
    if (appState !== 'dashboard' || !currentAgentId || !status?.birth_complete) {
      setRunningAgentRuns([]);
      setUpcomingOrchestrationJobs([]);
      return;
    }
    let cancelled = false;
    const loadOperations = async () => {
      try {
        const [runs, orchestration] = await Promise.all([
          fetchAgenticRuns(currentAgentId),
          fetchOrchestrationJobs(currentAgentId),
        ]);
        if (cancelled) return;
        setRunningAgentRuns(runs.filter((run) => run.status === 'running').slice(0, 4));
        setUpcomingOrchestrationJobs(
          (orchestration.jobs || [])
            .filter((job) => job.enabled && Boolean(job.next_run_at))
            .sort((a, b) => {
              const at = a.next_run_at ? new Date(a.next_run_at).getTime() : Number.MAX_SAFE_INTEGER;
              const bt = b.next_run_at ? new Date(b.next_run_at).getTime() : Number.MAX_SAFE_INTEGER;
              return at - bt;
            })
            .slice(0, 5)
        );
      } catch {
        if (!cancelled) {
          setRunningAgentRuns([]);
          setUpcomingOrchestrationJobs([]);
        }
      }
    };
    loadOperations();
    const interval = setInterval(loadOperations, 15000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [appState, currentAgentId, status?.birth_complete]);

  if (appState === 'splash') {
    return (
      <div className="app">
        <main className="main">
          <SplashScreen onComplete={handleSplashComplete} />
        </main>
        <StatusBar
          health={health}
          birthModelReady={null}
          localLlmConfigured={false}
          birthModelName={null}
        />
      </div>
    );
  }

  if (appState === 'hive') {
    return (
      <div className="app">
        <main className="main">
          <HiveScreen
            onAgentSelected={handleAgentSelected}
            onCreateAgent={handleCreateAgent}
            onViewIntro={() => setAppState('splash')}
          />
        </main>
        <StatusBar
          health={health}
          birthModelReady={status?.birth_model_ready ?? null}
          localLlmConfigured={status?.local_llm_configured ?? false}
          birthModelName={status?.birth_model ?? null}
        />
      </div>
    );
  }

  const phase = status?.birth_complete ? 'Operation' : 'Birth';
  const phaseMessage =
    status?.birth_complete
      ? OPERATION_MESSAGE
      : stageDisplayMessage(status?.birth_stage ?? null);

  // Show Darkness panel whenever we're in early birth (null or Darkness) so user always sees this step
  const showDarknessPanel =
    currentAgentId &&
    status &&
    !status.birth_complete &&
    (status.birth_stage == null || status.birth_stage === 'Darkness');
  const showIgnitionPanel =
    currentAgentId &&
    status &&
    !status.birth_complete &&
    status.birth_stage === 'Ignition';
  const showConnectivityPanel =
    currentAgentId &&
    status &&
    !status.birth_complete &&
    status.birth_stage === 'Connectivity' &&
    !connectivityDone &&
    !genesisPathStarted;
  const showPathSelector =
    status &&
    !status.birth_complete &&
    status.birth_stage === 'Connectivity' &&
    currentAgentId &&
    connectivityDone &&
    !genesisPathStarted;
  const showForgeScenario =
    currentAgentId &&
    genesisPathStarted === 'soul_forge' &&
    forgeInitial &&
    (status?.birth_stage === 'Genesis' || genesisPathStarted);

  const showGenesisChatPlaceholder =
    currentAgentId &&
    status &&
    !status.birth_complete &&
    status.birth_stage === 'Genesis' &&
    genesisPathStarted === 'soul_crystallization';

  const showGenesisChat =
    currentAgentId &&
    status &&
    !status.birth_complete &&
    status.birth_stage === 'Genesis' &&
    genesisPathStarted === 'direct';

  const showEmergencePanel =
    currentAgentId &&
    status &&
    !status.birth_complete &&
    status.birth_stage === 'Emergence';

  const displayModeProfile = routingTelemetry?.mode_profile
    ? routingTelemetry.mode_profile.charAt(0).toUpperCase() +
      routingTelemetry.mode_profile.slice(1)
    : null;
  const displayToolsProfile = routingTelemetry?.tools_profile
    ? routingTelemetry.tools_profile.charAt(0).toUpperCase() +
      routingTelemetry.tools_profile.slice(1)
    : null;

  const activeThinkingProvider =
    activeTierProvider && tierModels[activeTierProvider]
      ? activeTierProvider
      : Object.keys(tierModels)[0] ?? null;
  const activeThinkingModels = activeThinkingProvider
    ? tierModels[activeThinkingProvider]
    : null;

  return (
    <div className="app">
      <header className="header">
        <h1>Orion</h1>
        <span className={`badge badge-${health}`}>{health}</span>
        <button
          type="button"
          className="header-disconnect"
          onClick={handleDisconnect}
          title="Return to identity selector"
        >
          [disconnect]
        </button>
      </header>
      {error && <p className="error">{error}</p>}
      <main className="main">
        {showDarknessPanel && (
          <section className="panel birth-darkness-panel">
            <h2>Darkness</h2>
            <p className="phase-message">Save your identity key. You will only see it once.</p>
            {birthStateError && (
              <p className="error">
                {birthStateError}
                <button
                  type="button"
                  className="button-primary"
                  style={{ marginLeft: '0.75rem' }}
                  onClick={() => {
                    setBirthStateError(null);
                    setBirthState(null);
                  }}
                >
                  Retry
                </button>
              </p>
            )}
            {!birthStateError && birthStateLoading && (
              <p className="muted">Generating identity…</p>
            )}
            {!birthStateError && !birthStateLoading && birthState?.private_key_base64 && (
              <>
                <pre className="private-key-block" aria-label="Private key (base64)">
                  {birthState.private_key_base64}
                </pre>
                <p className="muted">Store this key securely. It cannot be recovered.</p>
                <button
                  type="button"
                  className="button-primary"
                  onClick={handleSavedKey}
                >
                  I&apos;ve saved the key
                </button>
              </>
            )}
            {!birthStateError && !birthStateLoading && birthState && !birthState.private_key_base64 && (
              <>
                <p className="muted">
                  Your identity key was generated in a previous session. If you saved it, continue. If not, you may need to recreate this agent.
                </p>
                <button
                  type="button"
                  className="button-primary"
                  onClick={handleSavedKey}
                >
                  I have my key &mdash; continue to Ignition
                </button>
              </>
            )}
          </section>
        )}
        {showIgnitionPanel && (
          <section className="panel birth-ignition-panel">
            <h2>Ignition</h2>
            <p className="phase-message">Configure your local LLM (e.g. Ollama). Use localhost or 127.0.0.1.</p>
            <form onSubmit={handleIgnitionSubmit}>
              <label htmlFor="ignition-url">Local LLM base URL (optional if already set)</label>
              <input
                id="ignition-url"
                type="url"
                placeholder="http://localhost:11434"
                value={ignitionUrl}
                onChange={(e) => setIgnitionUrl(e.target.value)}
                className="input-url"
              />
              <button type="submit" className="button-primary">
                Continue to Connectivity
              </button>
            </form>
          </section>
        )}
        {showConnectivityPanel && !showDarknessPanel && !showIgnitionPanel && (
          <section className="panel connectivity-stage-panel">
            <ConnectivityPanel
              agentId={currentAgentId!}
              onContinue={() => setConnectivityDone(true)}
              onError={setError}
            />
          </section>
        )}
        {showPathSelector && !showDarknessPanel && !showIgnitionPanel && (
          <section className="panel genesis-panel">
            <GenesisPathSelector
              agentId={currentAgentId}
              onStarted={handleGenesisStarted}
              onError={setError}
            />
          </section>
        )}
        {showForgeScenario && !showPathSelector && !showDarknessPanel && !showIgnitionPanel && (
          <section className="panel forge-panel">
            <ForgeScenario
              agentId={currentAgentId!}
              initialState={forgeInitial.state}
              initialPrompt={forgeInitial.prompt}
              initialChoices={forgeInitial.choices}
              onComplete={handleForgeComplete}
              onError={setError}
            />
          </section>
        )}
        {showEmergencePanel && !showForgeScenario && !showDarknessPanel && !showIgnitionPanel && (
          <section className="panel birth-emergence-panel">
            <h2>Emergence</h2>
            <p className="phase-message">
              Sign your constitutional documents and finalize birth. This cannot be undone.
            </p>
            <button
              type="button"
              className="button-primary"
              onClick={handleCompleteEmergence}
              disabled={emergenceBusy}
            >
              {emergenceBusy ? 'Finalizing…' : 'Finalize birth'}
            </button>
          </section>
        )}
        {showGenesisChat && !showForgeScenario && !showDarknessPanel && !showIgnitionPanel && (
          <section className="panel genesis-chat-panel">
            <h2>Genesis: Direct Discovery</h2>
            <p className="phase-message">
              Discover your agent&apos;s name, purpose, and personality through conversation.
            </p>
            <GenesisChat
              agentId={currentAgentId!}
              onCrystallized={() => {
                fetchStatus().then(setStatus).catch(() => {});
              }}
              onError={setError}
            />
          </section>
        )}
        {showGenesisChatPlaceholder && !showForgeScenario && !showEmergencePanel && (
          <section className="panel genesis-chat-panel">
            <h2>Genesis: Soul Crystallization</h2>
            <p className="phase-message">
              Discover your agent through depth-based psychometric profiling.
            </p>
            <CrystallizationChat
              agentId={currentAgentId!}
              onCrystallized={() => {
                fetchStatus().then(setStatus).catch(() => {});
              }}
              onError={setError}
            />
          </section>
        )}
        {!showConnectivityPanel && !showPathSelector && !showForgeScenario && !showDarknessPanel && !showIgnitionPanel && !showGenesisChatPlaceholder && !showGenesisChat && !showEmergencePanel && (
          <section className="panel phase-panel">
            <h2>{phase}</h2>
            <p className="phase-message">{status ? phaseMessage : 'Loading…'}</p>
            {status?.birth_complete && routingTelemetry && (
              <div className="operation-telemetry">
                <div className="operation-telemetry-summary">
                  <span className="operation-telemetry-label">Routing profile</span>
                  {displayModeProfile && (
                    <span className="operation-telemetry-pill">{displayModeProfile}</span>
                  )}
                  {displayToolsProfile && (
                    <span className="operation-telemetry-pill operation-telemetry-pill-muted">
                      {displayToolsProfile} tools
                    </span>
                  )}
                </div>
                <details className="operation-telemetry-details">
                  <summary>Details</summary>
                  <div className="operation-telemetry-grid">
                    <div>
                      <span>Router mode</span>
                      <code>{routingTelemetry.requested_router_mode}</code>
                    </div>
                    <div>
                      <span>Routing</span>
                      <code>{routingTelemetry.routing_mode}</code>
                    </div>
                    <div>
                      <span>Provider</span>
                      <code>{routingTelemetry.active_provider ?? 'none'}</code>
                    </div>
                    <div>
                      <span>Model</span>
                      <code>{routingTelemetry.active_model ?? 'unknown'}</code>
                    </div>
                    <div>
                      <span>Governor</span>
                      <code>{routingTelemetry.governor_enabled ? 'on' : 'off'}</code>
                    </div>
                  </div>
                </details>
              </div>
            )}
            {status?.birth_complete && (
              <div className="operation-snapshot">
                <div className="operation-snapshot-head">
                  <span>Operations</span>
                  <span>{runningAgentRuns.length} running / {upcomingOrchestrationJobs.length} upcoming</span>
                </div>
                <details className="operation-snapshot-details">
                  <summary>Running agent tasks</summary>
                  {runningAgentRuns.length === 0 ? (
                    <p className="muted">No active agentic runs.</p>
                  ) : (
                    <div className="operation-snapshot-list">
                      {runningAgentRuns.map((run) => (
                        <div key={run.task_id} className="operation-snapshot-item">
                          <strong>{run.goal}</strong>
                          <span>turns: {run.turns} / tools: {run.tool_calls}</span>
                        </div>
                      ))}
                    </div>
                  )}
                </details>
                <details className="operation-snapshot-details">
                  <summary>Upcoming scheduled jobs</summary>
                  {upcomingOrchestrationJobs.length === 0 ? (
                    <p className="muted">No enabled scheduled jobs.</p>
                  ) : (
                    <div className="operation-snapshot-list">
                      {upcomingOrchestrationJobs.map((job) => (
                        <div key={job.job_id} className="operation-snapshot-item">
                          <strong>{job.name}</strong>
                          <span>{job.next_run_at ? new Date(job.next_run_at).toLocaleString() : 'no next run'}</span>
                        </div>
                      ))}
                    </div>
                  )}
                </details>
              </div>
            )}
            {status && !status.birth_complete && status.birth_stage && (
              <p className="phase-stage">{status.birth_stage}</p>
            )}
            {status?.birth_complete && currentAgentId && !showChat && (
              <button
                type="button"
                className="button-primary"
                onClick={() => { setShowChat(true); setDashboardTab('chat'); }}
                style={{ marginTop: '1rem' }}
              >
                Talk to {status.agent_name || 'agent'}
              </button>
            )}
          </section>
        )}
        {showChat && status?.birth_complete && currentAgentId && (
          <>
            <nav className="dashboard-tabs">
              {(['chat', 'agent', 'jobs'] as const).map((tab) => (
                <button
                  key={tab}
                  className={`dashboard-tab${dashboardTab === tab ? ' dashboard-tab-active' : ''}`}
                  onClick={() => setDashboardTab(tab)}
                >
                  {tab === 'chat' ? 'Chat' : tab === 'agent' ? 'Agent' : 'Jobs'}
                </button>
              ))}
            </nav>
            {dashboardTab === 'chat' && (
              <section className="panel operational-chat-panel">
                <div className="chat-panel-header">
                  <h2 style={{ margin: 0 }}>
                    {chatMode === 'chat' ? `Chat with ${status.agent_name || 'agent'}` : 'Agentic Task'}
                  </h2>
                  <div className="chat-panel-controls">
                    <div className="router-mode-selector">
                      {(['auto', 'think_hard', 'think_harder'] as const).map((mode) => (
                        <button
                          key={mode}
                          className={`router-mode-pill${routerMode === mode ? ' router-mode-pill-active' : ''}`}
                          onClick={() => setRouterMode(mode)}
                        >
                          {mode === 'auto' ? 'Fast' : mode === 'think_hard' ? 'Standard' : 'Pro'}
                        </button>
                      ))}
                    </div>
                    <div className="chat-mode-toggle">
                      <button
                        className={chatMode === 'chat' ? 'button-primary' : 'button-secondary'}
                        onClick={() => setChatMode('chat')}
                        style={{ fontSize: '0.8rem', padding: '0.25rem 0.5rem' }}
                      >
                        Chat
                      </button>
                      <button
                        className={chatMode === 'agentic' ? 'button-primary' : 'button-secondary'}
                        onClick={() => setChatMode('agentic')}
                        style={{ fontSize: '0.8rem', padding: '0.25rem 0.5rem' }}
                      >
                        Agentic
                      </button>
                    </div>
                  </div>
                </div>
                {chatMode === 'chat' ? (
                  <OperationalChat
                    agentId={currentAgentId}
                    agentName={status.agent_name ?? undefined}
                    mentorName={mentorName ?? undefined}
                    onSetMentorName={handleSetMentorName}
                    routerMode={routerMode}
                    onError={setError}
                    onBusyChange={setChatBusy}
                    onRoutingTelemetryChange={setRoutingTelemetry}
                    onAgenticTaskLaunched={(info) => {
                      setLaunchedAgenticTask(info);
                      setDashboardTab('chat');
                      setChatMode('agentic');
                    }}
                  />
                ) : (
                  <AgenticPanel
                    agentId={currentAgentId}
                    agentName={status.agent_name ?? undefined}
                    routerMode={routerMode}
                    onRouterModeChange={setRouterMode}
                    onError={setError}
                    onBusyChange={setAgenticBusy}
                    externalTask={launchedAgenticTask}
                    onExternalTaskConsumed={() => setLaunchedAgenticTask(null)}
                  />
                )}
              </section>
            )}
            {dashboardTab === 'jobs' && (
              <>
                <section className="panel jobs-panel-section">
                  <JobsTable agentId={currentAgentId} />
                </section>
                <section className="panel orchestration-panel-section">
                  <OrchestrationJobsPanel agentId={currentAgentId} onError={setError} />
                </section>
              </>
            )}
            {dashboardTab === 'agent' && (
              <section className="panel status-panel">
                <h2>Agent Info</h2>
                {status ? (
                  <dl className="status">
                    <dt>Memory backend</dt>
                    <dd>{status.memory_backend}</dd>
                    <dt>Local LLM</dt>
                    <dd>{status.local_llm_configured ? 'Configured' : 'Not configured'}</dd>
                    <dt>Birth model</dt>
                    <dd>{status.birth_model ?? '—'}</dd>
                    <dt>Cloud providers</dt>
                    <dd>
                      <span className="provider-badges">
                        {storedProviders.map((p) => (
                          <span
                            key={p}
                            className={`provider-badge provider-badge-ok provider-badge-clickable${cloudBusy ? ' provider-badge-active' : ''}`}
                            title="Click to rotate key"
                            onClick={() => setKeyModalProvider(p)}
                          >
                            {p}
                            {cloudBusy && (
                              <span className="provider-dots">
                                <span />
                                <span />
                                <span />
                              </span>
                            )}
                          </span>
                        ))}
                        <span className="provider-add-key-wrap">
                          <button
                            type="button"
                            className="provider-add-key-btn"
                            onClick={() => setAddKeyPickerOpen((v) => !v)}
                          >
                            + Add key
                          </button>
                          {addKeyPickerOpen && (
                            <div className="provider-picker-dropdown">
                              {SUPPORTED_PROVIDERS.map((sp) => {
                                const configured = storedProviders.includes(sp.id);
                                return (
                                  <button
                                    key={sp.id}
                                    type="button"
                                    className={`provider-picker-item${configured ? ' configured' : ''}`}
                                    onClick={() => {
                                      setAddKeyPickerOpen(false);
                                      setKeyModalProvider(sp.id);
                                    }}
                                  >
                                    <span>{sp.label}</span>
                                    {configured && <span className="provider-picker-check">&#10003;</span>}
                                  </button>
                                );
                              })}
                            </div>
                          )}
                        </span>
                      </span>
                      {keyModalProvider && currentAgentId && (
                        <ApiKeyModal
                          agentId={currentAgentId}
                          provider={keyModalProvider}
                          onClose={() => setKeyModalProvider(null)}
                          onKeyStored={async (provider) => {
                            const storedProvider = provider;
                            setKeyModalProvider(null);
                            await refreshProviders();
                            if (currentAgentId) {
                              await setActiveProvider(currentAgentId, storedProvider);
                              const tm = await fetchTierModels(currentAgentId);
                              setTierModels(tm.models || {});
                              setActiveTierProvider(tm.active_provider ?? null);
                              setTierCatalog(tm.catalog || {});
                              setTierModelsDraft(tm.models || {});
                            }
                          }}
                          onError={(msg) => setError(msg)}
                        />
                      )}
                    </dd>
                    <dt>Thinking models</dt>
                    <dd>
                      {tierModelsLoading ? (
                        <span className="muted">Loading…</span>
                      ) : activeThinkingModels && activeThinkingProvider ? (
                        <div className="thinking-models-line">
                          <span className="provider-badge provider-badge-ok">
                            {activeThinkingProvider}
                          </span>
                          <span className="thinking-model-pill">
                            Fast: <code>{activeThinkingModels.fast}</code>
                          </span>
                          <span className="thinking-model-pill">
                            Standard: <code>{activeThinkingModels.standard}</code>
                          </span>
                          <span className="thinking-model-pill">
                            Pro: <code>{activeThinkingModels.pro}</code>
                          </span>
                        </div>
                      ) : (
                        <span className="muted">No LLM tier models available</span>
                      )}
                    </dd>
                    <dt>Model settings</dt>
                    <dd>
                      <div className="tier-model-actions">
                        <button
                          type="button"
                          className="tier-model-button"
                          onClick={() => {
                            setTierModelsDraft(tierModels);
                            setTierModelsEditOpen((prev) => !prev);
                          }}
                          disabled={tierModelsSaving}
                        >
                          {tierModelsEditOpen ? 'Hide editor' : 'Edit models'}
                        </button>
                        <button
                          type="button"
                          className="tier-model-button"
                          onClick={handleRefreshCatalog}
                          disabled={tierRefreshing}
                        >
                          {tierRefreshing ? 'Refreshing…' : 'Refresh catalogs'}
                        </button>
                        <button
                          type="button"
                          className="tier-model-button"
                          onClick={handleValidateModels}
                          disabled={tierValidating}
                        >
                          {tierValidating ? 'Validating…' : 'Validate models'}
                        </button>
                      </div>
                      {/* Catalog warnings */}
                      {Object.entries(tierCatalog).some(([, c]) => c.warnings.length > 0) && (
                        <div className="tier-model-warnings">
                          {Object.entries(tierCatalog).map(([prov, cat]) =>
                            cat.warnings.map((w, i) => (
                              <div key={`${prov}-${i}`} className="tier-model-warning">
                                {prov}: {w}
                              </div>
                            ))
                          )}
                        </div>
                      )}
                      {/* Validation results */}
                      {Object.keys(tierValidation).length > 0 && (
                        <div className="tier-model-validation-results">
                          {Object.entries(tierValidation).map(([prov, v]) => (
                            <div key={prov} className="tier-model-validation-row">
                              <span className="tier-model-validation-provider">{prov}</span>
                              {(['fast', 'standard', 'pro'] as const).map((t) => {
                                const r = v[t];
                                return (
                                  <span key={t} className={`tier-model-validation-badge ${r.valid ? 'valid' : 'invalid'}`}>
                                    {t}: {r.valid ? 'ok' : r.error ?? 'invalid'}
                                  </span>
                                );
                              })}
                            </div>
                          ))}
                        </div>
                      )}
                      {tierModelsEditOpen && (
                        <div className="tier-model-editor">
                          {Object.entries(tierModelsDraft).length === 0 ? (
                            <p className="muted">No configurable providers found.</p>
                          ) : (
                            Object.entries(tierModelsDraft).map(([provider, models]) => (
                              <div key={provider} className={`tier-model-provider${activeTierProvider === provider ? ' tier-model-provider-active' : ''}`}>
                                <div className="tier-model-provider-header">
                                  <p className="tier-model-provider-name">{provider}</p>
                                  <div className="tier-model-provider-actions">
                                    {activeTierProvider !== provider && (
                                      <button
                                        type="button"
                                        className="tier-model-button tier-model-button-sm"
                                        onClick={() => handleSetActiveProvider(provider)}
                                      >
                                        Set active
                                      </button>
                                    )}
                                    {activeTierProvider === provider && (
                                      <span className="tier-model-active-label">active</span>
                                    )}
                                    <button
                                      type="button"
                                      className="tier-model-button tier-model-button-sm tier-model-button-secondary"
                                      onClick={() => handleResetProviderModels(provider)}
                                    >
                                      Reset defaults
                                    </button>
                                  </div>
                                </div>
                                {tierCatalog[provider]?.last_refreshed && (
                                  <p className="tier-model-catalog-meta">
                                    Catalog: {tierCatalog[provider].source} ({tierCatalog[provider].available_models.length} models)
                                    {tierCatalog[provider].last_refreshed && ` — refreshed ${new Date(tierCatalog[provider].last_refreshed!).toLocaleString()}`}
                                  </p>
                                )}
                                <div className="tier-model-grid">
                                  <label>
                                    Fast
                                    <input
                                      type="text"
                                      value={models.fast}
                                      onChange={(e) =>
                                        handleTierModelDraftChange(
                                          provider,
                                          'fast',
                                          e.target.value
                                        )
                                      }
                                      list={`${provider}-models`}
                                    />
                                  </label>
                                  <label>
                                    Standard
                                    <input
                                      type="text"
                                      value={models.standard}
                                      onChange={(e) =>
                                        handleTierModelDraftChange(
                                          provider,
                                          'standard',
                                          e.target.value
                                        )
                                      }
                                      list={`${provider}-models`}
                                    />
                                  </label>
                                  <label>
                                    Pro
                                    <input
                                      type="text"
                                      value={models.pro}
                                      onChange={(e) =>
                                        handleTierModelDraftChange(
                                          provider,
                                          'pro',
                                          e.target.value
                                        )
                                      }
                                      list={`${provider}-models`}
                                    />
                                  </label>
                                </div>
                                {/* HTML datalist for autocomplete from catalog */}
                                {tierCatalog[provider]?.available_models.length > 0 && (
                                  <datalist id={`${provider}-models`}>
                                    {tierCatalog[provider].available_models.map((m) => (
                                      <option key={m} value={m} />
                                    ))}
                                  </datalist>
                                )}
                              </div>
                            ))
                          )}
                          {Object.entries(tierModelsDraft).length > 0 && (
                            <div className="tier-model-editor-actions">
                              <button
                                type="button"
                                className="tier-model-button"
                                onClick={handleSaveTierModels}
                                disabled={tierModelsSaving}
                              >
                                {tierModelsSaving ? 'Saving…' : 'Save'}
                              </button>
                              <button
                                type="button"
                                className="tier-model-button tier-model-button-secondary"
                                onClick={handleCancelTierModelsEdit}
                                disabled={tierModelsSaving}
                              >
                                Cancel
                              </button>
                            </div>
                          )}
                        </div>
                      )}
                    </dd>
                  </dl>
                ) : (
                  <p className="muted">Loading…</p>
                )}
              </section>
            )}
          </>
        )}
        {(!showChat || !status?.birth_complete) && (
          <section className="panel status-panel">
            <h2>Status</h2>
            {status ? (
              <dl className="status">
                <dt>Memory backend</dt>
                <dd>{status.memory_backend}</dd>
                <dt>Local LLM</dt>
                <dd>{status.local_llm_configured ? 'Configured' : 'Not configured'}</dd>
                <dt>Birth model</dt>
                <dd>{status.birth_model ?? '—'}</dd>
              </dl>
            ) : (
              <p className="muted">Loading…</p>
            )}
          </section>
        )}
      </main>
      <StatusBar
        health={health}
        birthModelReady={status?.birth_model_ready ?? null}
        localLlmConfigured={status?.local_llm_configured ?? false}
        birthModelName={status?.birth_model ?? null}
      />
    </div>
  );
}

export default App;
