import { useCallback, useEffect, useState } from 'react';
import {
  advanceDarkness,
  completeEmergence,
  fetchBirthState,
  fetchForgeState,
  fetchGenesisState,
  fetchHealth,
  fetchStatus,
  fetchStoredProviders,
  setIgnition,
  type BirthStateResponse,
  type StatusResponse,
} from './api';
import { stageDisplayMessage, OPERATION_MESSAGE } from './birthStages';
import SplashScreen from './components/SplashScreen';
import HiveScreen from './components/HiveScreen';
import GenesisPathSelector from './components/GenesisPathSelector';
import ForgeScenario from './components/ForgeScenario';
import GenesisChat from './components/GenesisChat';
import ConnectivityPanel from './components/ConnectivityPanel';
import OperationalChat from './components/OperationalChat';
import './App.css';

type AppState = 'splash' | 'hive' | 'dashboard';

function App() {
  const [appState, setAppState] = useState<AppState>('splash');
  const [currentAgentId, setCurrentAgentId] = useState<string | null>(null);
  const [health, setHealth] = useState<string>('pending');
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
  const [showChat, setShowChat] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const h = await fetchHealth();
        if (!cancelled) setHealth(h.status);
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
    if (appState !== 'dashboard') return;
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
    }, 3000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [appState]);

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
    const interval = setInterval(load, 5000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [appState, currentAgentId]);

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
    setShowChat(false);
  };

  const handleGenesisStarted = useCallback(
    async (
      path: string,
      data?: { state?: string; prompt?: string; choices?: string[] }
    ) => {
      setError(null);
      setGenesisPathStarted(path);
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

  if (appState === 'splash') {
    return <SplashScreen onComplete={handleSplashComplete} />;
  }

  if (appState === 'hive') {
    return (
      <HiveScreen
        onAgentSelected={handleAgentSelected}
        onCreateAgent={handleCreateAgent}
        onViewIntro={() => setAppState('splash')}
      />
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
                <p className="muted">Key was already shown. Continue to Ignition.</p>
                <button
                  type="button"
                  className="button-primary"
                  onClick={handleSavedKey}
                >
                  Continue to Ignition
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
          <section className="panel genesis-chat-placeholder-panel">
            <h2>Genesis: Soul Crystallization</h2>
            <p className="phase-message">
              The Q&A chat for this path is not yet available in the web UI.
            </p>
            <p className="muted">
              To complete birth with a guided flow now, disconnect and create or load an agent, then choose <strong>Soul Forge</strong> or <strong>Direct Discovery</strong> at the path step.
            </p>
          </section>
        )}
        {!showConnectivityPanel && !showPathSelector && !showForgeScenario && !showDarknessPanel && !showIgnitionPanel && !showGenesisChatPlaceholder && !showGenesisChat && !showEmergencePanel && (
          <section className="panel phase-panel">
            <h2>{phase}</h2>
            <p className="phase-message">{status ? phaseMessage : 'Loading…'}</p>
            {status && !status.birth_complete && status.birth_stage && (
              <p className="phase-stage">{status.birth_stage}</p>
            )}
            {status?.birth_complete && currentAgentId && !showChat && (
              <button
                type="button"
                className="button-primary"
                onClick={() => setShowChat(true)}
                style={{ marginTop: '1rem' }}
              >
                Talk to {status.agent_name || 'agent'}
              </button>
            )}
          </section>
        )}
        {showChat && status?.birth_complete && currentAgentId && (
          <section className="panel operational-chat-panel">
            <h2>Chat with {status.agent_name || 'agent'}</h2>
            <OperationalChat
              agentId={currentAgentId}
              agentName={status.agent_name ?? undefined}
              onError={setError}
            />
          </section>
        )}
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
              <dt>Cloud providers</dt>
              <dd>
                {storedProviders.length > 0 ? (
                  <span className="provider-badges">
                    {storedProviders.map((p) => (
                      <span key={p} className="provider-badge provider-badge-ok">
                        {p}
                      </span>
                    ))}
                  </span>
                ) : (
                  <span className="muted">None configured</span>
                )}
              </dd>
            </dl>
          ) : (
            <p className="muted">Loading…</p>
          )}
        </section>
      </main>
    </div>
  );
}

export default App;
