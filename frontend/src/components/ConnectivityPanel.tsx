import { useCallback, useEffect, useState } from 'react';
import {
  fetchConnectivityChatHistory,
  fetchStoredProviders,
  sendConnectivityChat,
  type BirthChatMessageItem,
} from '../api';
import { SUPPORTED_PROVIDERS } from '../providers';
import ApiKeyModal from './ApiKeyModal';
import './ConnectivityPanel.css';

interface ConnectivityPanelProps {
  agentId: string;
  onContinue: () => void;
  onError: (message: string) => void;
}

export default function ConnectivityPanel({
  agentId,
  onContinue,
  onError,
}: ConnectivityPanelProps) {
  const [messages, setMessages] = useState<BirthChatMessageItem[]>([]);
  const [storedProviders, setStoredProviders] = useState<string[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(true);
  const [sending, setSending] = useState(false);
  const [modalProvider, setModalProvider] = useState<string | null>(null);
  const [keyFlash, setKeyFlash] = useState<string | null>(null);

  // Load chat history and stored providers on mount
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [historyRes, providersRes] = await Promise.all([
          fetchConnectivityChatHistory(agentId),
          fetchStoredProviders(agentId),
        ]);
        if (!cancelled) {
          setMessages(historyRes.messages);
          setStoredProviders(providersRes.providers);
        }
      } catch {
        if (!cancelled) {
          setMessages([]);
          setStoredProviders([]);
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [agentId]);

  // Auto-scroll messages to bottom
  useEffect(() => {
    const el = document.querySelector('.connectivity-chat-messages');
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

  const handleSend = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      const text = input.trim();
      if (!text || sending) return;
      setInput('');
      setSending(true);
      const userMessage: BirthChatMessageItem = { role: 'user', content: text };
      setMessages((prev) => [...prev, userMessage]);
      try {
        const res = await sendConnectivityChat(agentId, text);
        setMessages((prev) => [
          ...prev,
          { role: 'assistant', content: res.assistant_content },
        ]);
        if (res.stored_providers) {
          setStoredProviders(res.stored_providers);
        }
        if (res.key_stored) {
          showKeyFlash(res.key_stored.provider);
        }
      } catch (err) {
        onError(err instanceof Error ? err.message : 'Send failed');
        // Remove optimistic user message on failure
        setMessages((prev) => prev.slice(0, -1));
      } finally {
        setSending(false);
      }
    },
    [agentId, input, sending, onError]
  );

  const showKeyFlash = (provider: string) => {
    setKeyFlash(`${provider.charAt(0).toUpperCase() + provider.slice(1)} key stored`);
    setTimeout(() => setKeyFlash(null), 3000);
  };

  const handleKeyStored = useCallback(
    (provider: string) => {
      setStoredProviders((prev) =>
        prev.includes(provider) ? prev : [...prev, provider]
      );
      showKeyFlash(provider);
      // Inject a system-like message so the LLM knows about the button-channel key
      setMessages((prev) => [
        ...prev,
        {
          role: 'user',
          content: `[${provider.toUpperCase()} API key has been saved via the provider button.]`,
        },
      ]);
    },
    []
  );

  if (loading) {
    return (
      <div className="connectivity-panel">
        <p className="muted">Loading connectivity state...</p>
      </div>
    );
  }

  return (
    <div className="connectivity-panel">
      <h2>Connectivity</h2>
      <p className="phase-message">
        Connect cloud AI providers to unlock your Ego. Enter keys via the buttons below or paste them in chat.
      </p>

      {/* Provider button row */}
      <div className="connectivity-providers">
        {SUPPORTED_PROVIDERS.map((p) => {
          const isStored = storedProviders.includes(p.id);
          return (
            <button
              key={p.id}
              type="button"
              className={`connectivity-provider-btn${isStored ? ' stored' : ''}`}
              onClick={() => setModalProvider(p.id)}
            >
              {isStored ? '\u2713 ' : ''}{p.label}
            </button>
          );
        })}
      </div>

      {keyFlash && <p className="connectivity-key-flash">{keyFlash}</p>}

      {/* Chat area */}
      <div className="connectivity-chat">
        <div className="connectivity-chat-messages">
          {messages.length === 0 && (
            <p className="muted" style={{ fontSize: '0.9rem' }}>
              Say hello to start the conversation, or use the buttons above to enter keys directly.
            </p>
          )}
          {messages.map((m, i) => (
            <div
              key={i}
              className={`connectivity-chat-message connectivity-chat-message-${m.role}`}
            >
              <span className="connectivity-chat-message-role">{m.role}</span>
              <div className="connectivity-chat-message-content">{m.content}</div>
            </div>
          ))}
        </div>
        <form onSubmit={handleSend} className="connectivity-chat-form">
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder="Type your message..."
            className="connectivity-chat-input"
            disabled={sending}
          />
          <button
            type="submit"
            className="button-primary connectivity-chat-send"
            disabled={sending || !input.trim()}
          >
            {sending ? 'Sending...' : 'Send'}
          </button>
        </form>
      </div>

      {/* Continue / Skip actions */}
      <div className="connectivity-actions">
        {storedProviders.length > 0 ? (
          <button
            type="button"
            className="button-primary"
            onClick={onContinue}
          >
            Continue to Genesis &gt;
          </button>
        ) : (
          <button
            type="button"
            className="button-secondary"
            onClick={onContinue}
          >
            Skip &mdash; continue with local LLM only
          </button>
        )}
      </div>

      {/* API Key Modal */}
      {modalProvider && (
        <ApiKeyModal
          agentId={agentId}
          provider={modalProvider}
          onClose={() => setModalProvider(null)}
          onKeyStored={handleKeyStored}
          onError={onError}
        />
      )}
    </div>
  );
}
