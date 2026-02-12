import { useState } from 'react';
import { forgeCrystallize, forgeSelect } from '../api';
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
    crystallized?: boolean;
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
  const [, setSelected] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{
    archetype: string;
    soul_hash?: string;
    sigil_art?: string;
    weights?: Record<string, number>;
  } | null>(null);
  const [crystallizeName, setCrystallizeName] = useState('');
  const [crystallizePurpose, setCrystallizePurpose] = useState('');
  const [crystallizePersonality, setCrystallizePersonality] = useState('');
  const [crystallizeBusy, setCrystallizeBusy] = useState(false);

  const handleChoice = async (index: number) => {
    if (busy) return;
    setSelected(index);
    setBusy(true);
    try {
      const data = await forgeSelect(agentId, index);
      setState(data.state);
      if (data.state === 'crystallize' || data.state === 'done') {
        // Store result locally — the crystallize form will show.
        // Do NOT call onComplete yet; that happens after the user submits
        // the name/purpose/personality form (handleCrystallizeSubmit).
        setResult({
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

  const handleCrystallizeSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const name = crystallizeName.trim();
    if (!name || crystallizeBusy) return;
    setCrystallizeBusy(true);
    try {
      await forgeCrystallize(agentId, {
        name,
        purpose: crystallizePurpose.trim() || undefined,
        personality: crystallizePersonality.trim() || undefined,
      });
      onComplete({
        ...result!,
        crystallized: true,
      });
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Crystallize failed');
    } finally {
      setCrystallizeBusy(false);
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
        <form onSubmit={handleCrystallizeSubmit} className="forge-result-form">
          <label htmlFor="forge-name">Agent name (required)</label>
          <input
            id="forge-name"
            type="text"
            value={crystallizeName}
            onChange={(e) => setCrystallizeName(e.target.value)}
            placeholder="e.g. Orion"
            className="forge-result-input"
            required
          />
          <label htmlFor="forge-purpose">Purpose (optional)</label>
          <input
            id="forge-purpose"
            type="text"
            value={crystallizePurpose}
            onChange={(e) => setCrystallizePurpose(e.target.value)}
            placeholder="What your agent is for"
            className="forge-result-input"
          />
          <label htmlFor="forge-personality">Personality (optional)</label>
          <input
            id="forge-personality"
            type="text"
            value={crystallizePersonality}
            onChange={(e) => setCrystallizePersonality(e.target.value)}
            placeholder="Tone and style"
            className="forge-result-input"
          />
          <button
            type="submit"
            className="button-primary forge-result-submit"
            disabled={crystallizeBusy || !crystallizeName.trim()}
          >
            {crystallizeBusy ? 'Crystallizing…' : 'Crystallize soul'}
          </button>
        </form>
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
