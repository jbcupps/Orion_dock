import { useEffect, useState } from 'react';
import { fetchHealth, fetchStatus, type StatusResponse } from './api';
import { stageDisplayMessage, OPERATION_MESSAGE } from './birthStages';
import SplashScreen from './components/SplashScreen';
import HiveScreen from './components/HiveScreen';
import GenesisPathSelector from './components/GenesisPathSelector';
import ForgeScenario from './components/ForgeScenario';
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
  };

  const handleGenesisStarted = (
    path: string,
    data?: { state?: string; prompt?: string; choices?: string[] }
  ) => {
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
  };

  const handleForgeComplete = () => {
    setForgeInitial(null);
  };

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

  const showPathSelector =
    status && !status.birth_complete && status.birth_stage === 'Connectivity' && currentAgentId;
  const showForgeScenario =
    currentAgentId &&
    genesisPathStarted === 'soul_forge' &&
    forgeInitial &&
    (status?.birth_stage === 'Genesis' || genesisPathStarted);

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
        {showPathSelector && (
          <section className="panel genesis-panel">
            <GenesisPathSelector
              agentId={currentAgentId}
              onStarted={handleGenesisStarted}
              onError={setError}
            />
          </section>
        )}
        {showForgeScenario && !showPathSelector && (
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
        {!showPathSelector && !showForgeScenario && (
          <>
            <section className="panel phase-panel">
              <h2>{phase}</h2>
              <p className="phase-message">{status ? phaseMessage : 'Loading…'}</p>
              {status && !status.birth_complete && status.birth_stage && (
                <p className="phase-stage">{status.birth_stage}</p>
              )}
            </section>
            <section className="panel">
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
          </>
        )}
      </main>
    </div>
  );
}

export default App;
