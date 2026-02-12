import { useCallback, useEffect, useRef, useState } from 'react';
import {
  fetchChatHistory,
  sendChat,
  type BirthChatMessageItem,
} from '../api';
import './OperationalChat.css';

interface OperationalChatProps {
  agentId: string;
  agentName?: string;
  onError: (message: string) => void;
}

export default function OperationalChat({
  agentId,
  agentName,
  onError,
}: OperationalChatProps) {
  const [messages, setMessages] = useState<BirthChatMessageItem[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(true);
  const [sending, setSending] = useState(false);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const { messages: history } = await fetchChatHistory(agentId);
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
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
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
        const res = await sendChat(agentId, text);
        setMessages((prev) => [
          ...prev,
          { role: 'assistant', content: res.assistant_content },
        ]);
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Send failed');
        setMessages((prev) => prev.slice(0, -1));
      } finally {
        setSending(false);
      }
    },
    [agentId, input, sending, onError]
  );

  if (loading) {
    return (
      <div className="operational-chat">
        <p className="operational-chat-loading">Loading conversation…</p>
      </div>
    );
  }

  const displayName = agentName || 'your agent';

  return (
    <div className="operational-chat">
      <div className="operational-chat-messages">
        {messages.length === 0 && (
          <p className="operational-chat-empty">
            Say hello to {displayName}. Start a conversation.
          </p>
        )}
        {messages.map((m, i) => (
          <div
            key={i}
            className={`operational-chat-message operational-chat-message-${m.role}`}
          >
            <span className="operational-chat-message-role">
              {m.role === 'assistant' ? (agentName || 'assistant') : m.role}
            </span>
            <div className="operational-chat-message-content">{m.content}</div>
          </div>
        ))}
        <div ref={messagesEndRef} />
      </div>
      <form onSubmit={handleSend} className="operational-chat-form">
        <input
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder={`Talk to ${displayName}…`}
          className="operational-chat-input"
          disabled={sending}
        />
        <button
          type="submit"
          className="button-primary operational-chat-send"
          disabled={sending || !input.trim()}
        >
          {sending ? 'Sending…' : 'Send'}
        </button>
      </form>
    </div>
  );
}
