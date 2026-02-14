import { useCallback, useEffect, useRef, useState } from 'react';
import {
  fetchChatHistory,
  sendChat,
  uploadChatAttachments,
  type BirthChatMessageItem,
  type ChatAttachmentUploadItem,
  type OperationalChatResponse,
} from '../api';
import './OperationalChat.css';

interface OperationalChatProps {
  agentId: string;
  agentName?: string;
  routerMode?: 'auto' | 'think_hard' | 'think_harder';
  onError: (message: string) => void;
  onBusyChange?: (busy: boolean) => void;
}

function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

function renderUserMessage(
  text: string,
  attachments: ChatAttachmentUploadItem[]
): string {
  if (!attachments.length) return text;
  const lines = attachments.map(
    (a) => `- ${a.file_name} (${a.detected_kind}, ${formatBytes(a.size_bytes)})`
  );
  if (!text) {
    return `[Attachments]\n${lines.join('\n')}`;
  }
  return `${text}\n\n[Attachments]\n${lines.join('\n')}`;
}

export default function OperationalChat({
  agentId,
  agentName,
  routerMode,
  onError,
  onBusyChange,
}: OperationalChatProps) {
  type ActivityLogItem = NonNullable<OperationalChatResponse['tool_log']>[number] & {
    id: string;
  };
  const [messages, setMessages] = useState<BirthChatMessageItem[]>([]);
  const [activityLog, setActivityLog] = useState<ActivityLogItem[]>([]);
  const [input, setInput] = useState('');
  const [attachments, setAttachments] = useState<ChatAttachmentUploadItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [sending, setSending] = useState(false);
  const [uploadingAttachments, setUploadingAttachments] = useState(false);
  const [dragActive, setDragActive] = useState(false);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

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

  // Propagate busy state to parent for provider activity indicators
  useEffect(() => {
    onBusyChange?.(sending || uploadingAttachments);
  }, [sending, uploadingAttachments, onBusyChange]);

  const handleFilesPicked = useCallback(
    async (incoming: FileList | File[]) => {
      const files = Array.from(incoming || []);
      if (!files.length) return;
      setUploadingAttachments(true);
      try {
        const res = await uploadChatAttachments(agentId, files);
        if (!res.attachments.length) return;
        setAttachments((prev) => [...prev, ...res.attachments]);
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Attachment upload failed');
      } finally {
        setUploadingAttachments(false);
      }
    },
    [agentId, onError]
  );

  const handleAttachmentInputChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const files = e.target.files;
      e.target.value = '';
      if (!files) return;
      void handleFilesPicked(files);
    },
    [handleFilesPicked]
  );

  const removeAttachment = useCallback((attachmentId: string) => {
    setAttachments((prev) =>
      prev.filter((item) => item.attachment_id !== attachmentId)
    );
  }, []);

  const handleSend = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      const text = input.trim();
      const attachmentIds = attachments.map((a) => a.attachment_id);
      if ((!text && attachmentIds.length === 0) || sending || uploadingAttachments)
        return;
      setInput('');
      setSending(true);
      const userMessage: BirthChatMessageItem = {
        role: 'user',
        content: renderUserMessage(text, attachments),
      };
      setMessages((prev) => [...prev, userMessage]);
      try {
        const outboundMessage =
          text || 'Please analyze the uploaded attachments.';
        const res = await sendChat(
          agentId,
          outboundMessage,
          routerMode,
          attachmentIds
        );
        const assistantContent = res.attachment_notice
          ? `${res.assistant_content}\n\n${res.attachment_notice}`
          : res.assistant_content;
        setMessages((prev) => [
          ...prev,
          { role: 'assistant', content: assistantContent },
        ]);
        setAttachments([]);
        if (res.tool_log?.length) {
          const entries: ActivityLogItem[] = res.tool_log.map((entry, index) => ({
            ...entry,
            id: `${Date.now()}-${index}-${entry.tool_name}`,
          }));
          setActivityLog((prev) => [...entries, ...prev].slice(0, 20));
        }
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Send failed');
        setMessages((prev) => prev.slice(0, -1));
      } finally {
        setSending(false);
      }
    },
    [
      agentId,
      attachments,
      input,
      onError,
      routerMode,
      sending,
      uploadingAttachments,
    ]
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
      <details className="operational-chat-activity">
        <summary>
          Activity log
          {activityLog.length > 0 ? ` (${activityLog.length})` : ''}
        </summary>
        {activityLog.length === 0 ? (
          <p className="operational-chat-activity-empty">No tool activity yet.</p>
        ) : (
          <div className="operational-chat-activity-list">
            {activityLog.map((entry) => (
              <div key={entry.id} className="operational-chat-activity-item">
                <div className="operational-chat-activity-title">
                  <span>{entry.tool_name}</span>
                  <span className={entry.success ? 'status-ok' : 'status-error'}>
                    {entry.success ? 'ok' : 'error'}
                  </span>
                </div>
                {entry.skill_name && (
                  <div className="operational-chat-activity-skill">{entry.skill_name}</div>
                )}
                <pre className="operational-chat-activity-output">{entry.output}</pre>
              </div>
            ))}
          </div>
        )}
      </details>
      <div
        className={`operational-chat-composer${dragActive ? ' drag-active' : ''}`}
        onDragOver={(e) => {
          e.preventDefault();
          if (!dragActive) setDragActive(true);
        }}
        onDragLeave={(e) => {
          e.preventDefault();
          if (dragActive) setDragActive(false);
        }}
        onDrop={(e) => {
          e.preventDefault();
          setDragActive(false);
          void handleFilesPicked(e.dataTransfer.files);
        }}
      >
        <input
          ref={fileInputRef}
          type="file"
          multiple
          className="operational-chat-file-input"
          onChange={handleAttachmentInputChange}
          disabled={sending || uploadingAttachments}
        />
        {attachments.length > 0 && (
          <div className="operational-chat-attachments">
            {attachments.map((item) => (
              <div key={item.attachment_id} className="operational-chat-attachment-chip">
                <div className="operational-chat-attachment-main">
                  <span className="operational-chat-attachment-name">{item.file_name}</span>
                  <span className="operational-chat-attachment-meta">
                    {item.detected_kind} • {formatBytes(item.size_bytes)} • {item.parse_status}
                  </span>
                </div>
                <button
                  type="button"
                  className="operational-chat-attachment-remove"
                  onClick={() => removeAttachment(item.attachment_id)}
                  disabled={sending || uploadingAttachments}
                  aria-label={`Remove ${item.file_name}`}
                >
                  ×
                </button>
              </div>
            ))}
          </div>
        )}
        {uploadingAttachments && (
          <p className="operational-chat-uploading">Uploading attachments…</p>
        )}
        <form onSubmit={handleSend} className="operational-chat-form">
          <button
            type="button"
            className="button-secondary operational-chat-attach"
            onClick={() => fileInputRef.current?.click()}
            disabled={sending || uploadingAttachments}
          >
            Attach
          </button>
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder={`Talk to ${displayName}…`}
            className="operational-chat-input"
            disabled={sending || uploadingAttachments}
          />
          <button
            type="submit"
            className="button-primary operational-chat-send"
            disabled={
              sending ||
              uploadingAttachments ||
              (!input.trim() && attachments.length === 0)
            }
          >
            {sending ? 'Sending…' : 'Send'}
          </button>
        </form>
        <p className="operational-chat-drop-hint">
          Drag and drop files here or use Attach. Files are analyzed only and not executed.
        </p>
      </div>
    </div>
  );
}
