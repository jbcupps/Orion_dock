import { useEffect, useRef, useState } from 'react';
import { genesisStep, type GenesisStepRequest, type GenesisStepResponse } from '../api';
import './GenesisChat.css';

interface GenesisStepViewProps {
  agentId: string;
  initialStep: GenesisStepRequest;
  onComplete: () => void;
  onError: (message: string) => void;
}

/**
 * Unified genesis step view that handles all StepRequest types:
 * - NeedUserMessage: free-form chat input
 * - NeedChoice: discrete choice buttons
 * - NeedForm: structured form fields
 * - Complete: signals completion to parent
 */
export default function GenesisStepView({
  agentId,
  initialStep,
  onComplete,
  onError,
}: GenesisStepViewProps) {
  const [currentStep, setCurrentStep] = useState<GenesisStepRequest>(initialStep);
  const [busy, setBusy] = useState(false);
  const [chatInput, setChatInput] = useState('');
  const [formValues, setFormValues] = useState<Record<string, string>>({});
  const messagesEndRef = useRef<HTMLDivElement>(null);

  // Auto-scroll on step changes
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [currentStep]);

  // Handle completion
  useEffect(() => {
    if (currentStep.type === 'Complete') {
      onComplete();
    }
  }, [currentStep, onComplete]);

  const sendStep = async (response: GenesisStepResponse) => {
    if (busy) return;
    setBusy(true);
    try {
      const nextStep = await genesisStep(agentId, response);
      setCurrentStep(nextStep);
      setChatInput('');
      setFormValues({});
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Genesis step failed');
    } finally {
      setBusy(false);
    }
  };

  const handleChatSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const text = chatInput.trim();
    if (!text) return;
    sendStep({ type: 'Message', text });
  };

  const handleChoice = (index: number) => {
    sendStep({ type: 'Choice', index });
  };

  const handleFormSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    sendStep({ type: 'Form', values: { ...formValues } });
  };

  if (currentStep.type === 'Complete') {
    const { manifest } = currentStep;
    return (
      <div className="genesis-chat">
        <div className="genesis-step-complete">
          <h3>Genesis Complete</h3>
          <p><strong>Archetype:</strong> {manifest.archetype}</p>
          {manifest.sigil_art && (
            <pre className="forge-result-sigil">{manifest.sigil_art}</pre>
          )}
          <p><strong>Birth Method:</strong> {manifest.birth_method}</p>
          <p><small>Hash: {manifest.genesis_hash.slice(0, 16)}...</small></p>
        </div>
      </div>
    );
  }

  if (currentStep.type === 'NeedChoice') {
    const { prompt, choices, metadata } = currentStep;
    const scenarioIndex = metadata?.scenario_index as number | undefined;
    const scenarioTotal = metadata?.scenario_total as number | undefined;
    const title =
      scenarioIndex !== undefined && scenarioTotal !== undefined
        ? `Scenario ${scenarioIndex + 1} of ${scenarioTotal}`
        : undefined;

    return (
      <div className="genesis-chat">
        {title && <h3 className="forge-scenario-title">{title}</h3>}
        <div className="genesis-step-prompt">{prompt}</div>
        <div className="genesis-step-choices">
          {choices.map((choice, i) => (
            <button
              key={i}
              type="button"
              className="genesis-step-choice-btn"
              onClick={() => handleChoice(i)}
              disabled={busy}
            >
              <span className="genesis-step-choice-label">{choice.label}</span>
              {choice.description && (
                <span className="genesis-step-choice-desc">{choice.description}</span>
              )}
            </button>
          ))}
        </div>
        {busy && <p className="forge-scenario-busy">Processing...</p>}
        <div ref={messagesEndRef} />
      </div>
    );
  }

  if (currentStep.type === 'NeedUserMessage') {
    const { prompt, conversation } = currentStep;
    return (
      <div className="genesis-chat">
        <div className="genesis-chat-messages">
          {conversation.map((msg, i) => (
            <div
              key={i}
              className={`genesis-chat-message genesis-chat-message-${msg.role}`}
            >
              <span className="genesis-chat-role">{msg.role}</span>
              <span className="genesis-chat-content">{msg.content}</span>
            </div>
          ))}
          {prompt && (
            <div className="genesis-chat-message genesis-chat-message-assistant">
              <span className="genesis-chat-role">assistant</span>
              <span className="genesis-chat-content">{prompt}</span>
            </div>
          )}
          <div ref={messagesEndRef} />
        </div>
        <form onSubmit={handleChatSubmit} className="genesis-chat-form">
          <input
            type="text"
            value={chatInput}
            onChange={(e) => setChatInput(e.target.value)}
            placeholder="Type your response..."
            className="genesis-chat-input"
            disabled={busy}
          />
          <button
            type="submit"
            className="genesis-chat-send"
            disabled={busy || !chatInput.trim()}
          >
            {busy ? 'Sending...' : 'Send'}
          </button>
        </form>
      </div>
    );
  }

  if (currentStep.type === 'NeedForm') {
    const { prompt, fields } = currentStep;
    return (
      <div className="genesis-chat">
        <div className="genesis-step-prompt">{prompt}</div>
        <form onSubmit={handleFormSubmit} className="genesis-step-form">
          {fields.map((field) => (
            <div key={field.name} className="genesis-step-form-field">
              <label htmlFor={`genesis-field-${field.name}`}>{field.label}</label>
              <input
                id={`genesis-field-${field.name}`}
                type="text"
                value={formValues[field.name] ?? field.default_value ?? ''}
                onChange={(e) =>
                  setFormValues((prev) => ({ ...prev, [field.name]: e.target.value }))
                }
                required={field.required}
                className="forge-result-input"
              />
            </div>
          ))}
          <button
            type="submit"
            className="button-primary"
            disabled={busy}
          >
            {busy ? 'Submitting...' : 'Continue'}
          </button>
        </form>
        <div ref={messagesEndRef} />
      </div>
    );
  }

  return null;
}
