import { useState } from 'react';
import { forgeSelect } from '../api';
import './ForgeScenario.css';

interface ForgeScenarioProps {
  agentId: string;
  initialState?: string;
  initialPrompt?: string;
  initialChoices?: string[];
  onComplete: (result: {
    archetype: string;
    soul_hash?: string;
    sigil_art?: string;
    weights?: Record<string, number>;
  }) => void;
  onError: (message: string) => void;
}

export default function ForgeScenario({
  agentId,
  initialState,
  initialPrompt,
  initialChoices,
  onComplete,
  onError,
}: ForgeScenarioProps) {
  const [state, setState] = useState(initialState ?? 'scenario1');
  const [prompt, setPrompt] = useState(initialPrompt ?? '');
  const [choices, setChoices] = useState<string[]>(initialChoices ?? []);
  const [selected, setSelected] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{
    archetype: string;
    soul_hash?: string;
    sigil_art?: string;
    weights?: Record<string, number>;
  } | null>(null);

  const handleChoice = async (index: number) => {
    if (busy) return;
    setSelected(index);
    setBusy(true);
    try {
      const data = await forgeSelect(agentId, index);
      setState(data.state);
      if (data.state === 'crystallize' || data.state === 'done') {
        setResult({
          archetype: data.archetype ?? '',
          soul_hash: data.soul_hash,
          sigil_art: data.sigil_art,
          weights: data.weights,
        });
        onComplete({
          archetype: data.archetype ?? '',
          soul_hash: data.soul_hash,
          sigil_art: data.sigil_art,
          weights: data.weights,
        });
      } else {
        setPrompt(data.prompt ?? '');
        setChoices(data.choices ?? []);
        setSelected(null);
      }
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Selection failed');
    } finally {
      setBusy(false);
    }
  };

  if (result) {
    return (
      <div className="forge-result">
        <h3 className="forge-result-title">Soul Forge complete</h3>
        <p className="forge-result-archetype">{result.archetype}</p>
        {result.sigil_art && (
          <pre className="forge-result-sigil">{result.sigil_art}</pre>
        )}
        {result.soul_hash && (
          <p className="forge-result-hash">
            <small>Hash: {result.soul_hash.slice(0, 16)}…</small>
          </p>
        )}
        <p className="forge-result-next">
          Next: provide a name for your agent to crystallize the soul document.
        </p>
      </div>
    );
  }

  const scenarioNum =
    state === 'scenario1' ? 1 : state === 'scenario2' ? 2 : state === 'scenario3' ? 3 : 0;

  return (
    <div className="forge-scenario">
      <h3 className="forge-scenario-title">Calibration {scenarioNum}/3</h3>
      <div className="forge-scenario-prompt">{prompt}</div>
      <div className="forge-scenario-choices">
        {choices.map((choice, i) => (
          <button
            key={i}
            type="button"
            className="forge-scenario-choice"
            onClick={() => handleChoice(i)}
            disabled={busy}
          >
            {choice}
          </button>
        ))}
      </div>
      {busy && <p className="forge-scenario-busy">Updating…</p>}
    </div>
  );
}
