import { useCallback, useEffect, useState } from 'react';
import {
  fetchBirthChatHistory,
  sendBirthChat,
  type BirthChatMessageItem,
} from '../api';
import './GenesisChat.css';

interface CrystallizationChatProps {
  agentId: string;
  onCrystallized?: () => void;
  onError: (message: string) => void;
}

export default function CrystallizationChat({
  agentId,
  onCrystallized,
  onError,
}: CrystallizationChatProps) {
  const [messages, setMessages] = useState<BirthChatMessageItem[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(true);
  const [sending, setSending] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const { messages: history } = await fetchBirthChatHistory(agentId);
        if (!cancelled) setMessages(history);
      } catch {
        if (!cancelled) setMessages([]);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [agentId]);

  useEffect(() => {
    const el = document.querySelector('.genesis-chat-messages');
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
        const res = await sendBirthChat(agentId, text);
        setMessages((prev) => [
          ...prev,
          { role: 'assistant', content: res.assistant_content },
        ]);
        if (res.crystallized) {
          onCrystallized?.();
        }
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Send failed');
        setMessages((prev) => prev.slice(0, -1));
      } finally {
        setSending(false);
      }
    },
    [agentId, input, sending, onCrystallized, onError]
  );

  if (loading) {
    return (
      <div className="genesis-chat">
        <p className="genesis-chat-loading">Loading conversation...</p>
      </div>
    );
  }

  return (
    <div className="genesis-chat">
      <div className="genesis-chat-messages">
        {messages.length === 0 && (
          <p className="genesis-chat-empty">
            Say hello to begin the soul crystallization process. The depth of your conversation shapes the agent's identity.
          </p>
        )}
        {messages.map((m, i) => (
          <div
            key={i}
            className={`genesis-chat-message genesis-chat-message-${m.role}`}
          >
            <span className="genesis-chat-message-role">{m.role}</span>
            <div className="genesis-chat-message-content">{m.content}</div>
          </div>
        ))}
      </div>
      <form onSubmit={handleSend} className="genesis-chat-form">
        <input
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="Type your message..."
          className="genesis-chat-input"
          disabled={sending}
        />
        <button
          type="submit"
          className="button-primary genesis-chat-send"
          disabled={sending || !input.trim()}
        >
          {sending ? 'Sending...' : 'Send'}
        </button>
      </form>
    </div>
  );
}
