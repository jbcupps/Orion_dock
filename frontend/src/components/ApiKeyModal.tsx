import { useCallback, useState } from 'react';
import { storeProviderKey } from '../api';

const PROVIDER_PLACEHOLDERS: Record<string, string> = {
  openai: 'sk-...',
  anthropic: 'sk-ant-...',
  perplexity: 'pplx-...',
  xai: 'xai-...',
  google: 'AIza...',
  tavily: 'tvly-...',
};

interface ApiKeyModalProps {
  agentId: string;
  provider: string;
  onClose: () => void;
  onKeyStored: (provider: string) => void;
  onError: (message: string) => void;
}

export default function ApiKeyModal({
  agentId,
  provider,
  onClose,
  onKeyStored,
  onError,
}: ApiKeyModalProps) {
  const [key, setKey] = useState('');
  const [validate, setValidate] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const [validationFailed, setValidationFailed] = useState(false);

  const doStore = useCallback(
    async (shouldValidate: boolean) => {
      const trimmed = key.trim();
      if (!trimmed || submitting) return;
      setLocalError(null);
      setSubmitting(true);
      setValidationFailed(false);
      try {
        const res = await storeProviderKey(agentId, provider, trimmed, shouldValidate);
        onKeyStored(res.provider);
        onClose();
      } catch (err) {
        const msg = err instanceof Error ? err.message : 'Failed to store key';
        setLocalError(msg);
        if (shouldValidate) {
          setValidationFailed(true);
        } else {
          onError(msg);
        }
      } finally {
        setSubmitting(false);
      }
    },
    [agentId, provider, key, submitting, onClose, onKeyStored, onError]
  );

  const handleSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      doStore(validate);
    },
    [doStore, validate]
  );

  const placeholder = PROVIDER_PLACEHOLDERS[provider] || 'API key';

  return (
    <div className="api-key-modal-overlay" onClick={onClose}>
      <div
        className="api-key-modal"
        onClick={(e) => e.stopPropagation()}
      >
        <h3>{provider.charAt(0).toUpperCase() + provider.slice(1)} API Key</h3>
        <form onSubmit={handleSubmit}>
          <input
            type="password"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder={placeholder}
            className="api-key-modal-input"
            autoFocus
            disabled={submitting}
          />
          <label className="api-key-modal-validate">
            <input
              type="checkbox"
              checked={validate}
              onChange={(e) => setValidate(e.target.checked)}
              disabled={submitting}
            />
            Validate key before saving
          </label>
          {localError && <p className="api-key-modal-error">{localError}</p>}
          {validationFailed && (
            <button
              type="button"
              className="button-secondary"
              style={{ width: '100%', marginTop: '0.5rem' }}
              onClick={() => doStore(false)}
              disabled={submitting}
            >
              Save without validation
            </button>
          )}
          <div className="api-key-modal-actions">
            <button
              type="button"
              className="button-secondary"
              onClick={onClose}
              disabled={submitting}
            >
              Cancel
            </button>
            <button
              type="submit"
              className="button-primary"
              disabled={submitting || !key.trim()}
            >
              {submitting ? 'Saving...' : 'Save Key'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
