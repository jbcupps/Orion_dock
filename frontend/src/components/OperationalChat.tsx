import { useCallback, useEffect, useRef, useState } from 'react';
import {
  archiveChat,
  fetchChatArchive,
  fetchChatArchives,
  fetchChatHistory,
  sendChatStream,
  uploadChatAttachments,
  type BirthChatMessageItem,
  type ChatArchiveInfo,
  type ChatAttachmentUploadItem,
  type OperationalChatResponse,
  type RoutingTelemetry,
} from '../api';
import './OperationalChat.css';

interface OperationalChatProps {
  agentId: string;
  agentName?: string;
  mentorName?: string;
  onSetMentorName?: (name: string) => void;
  routerMode?: 'auto' | 'think_hard' | 'think_harder';
  onError: (message: string) => void;
  onBusyChange?: (busy: boolean) => void;
  onRoutingTelemetryChange?: (telemetry: RoutingTelemetry | null) => void;
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
  mentorName,
  onSetMentorName,
  routerMode,
  onError,
  onBusyChange,
  onRoutingTelemetryChange,
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
  const [imagePreviews, setImagePreviews] = useState<Record<string, string>>({});
  const [contextWarning, setContextWarning] = useState<string | null>(null);
  const [archives, setArchives] = useState<ChatArchiveInfo[]>([]);
  const [archivesOpen, setArchivesOpen] = useState(false);
  const [viewingArchive, setViewingArchive] = useState<{
    id: string;
    messages: BirthChatMessageItem[];
  } | null>(null);
  const [archiving, setArchiving] = useState(false);
  const [thinkingStatus, setThinkingStatus] = useState<string | null>(null);
  const [showMentorPrompt, setShowMentorPrompt] = useState(!mentorName);
  const [mentorDraft, setMentorDraft] = useState('');
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
        const previews: Record<string, string> = {};
        res.attachments.forEach((att, i) => {
          const file = files[i];
          if (file && file.type.startsWith('image/')) {
            previews[att.attachment_id] = URL.createObjectURL(file);
          }
        });
        if (Object.keys(previews).length > 0) {
          setImagePreviews((prev) => ({ ...prev, ...previews }));
        }
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

  const handlePaste = useCallback(
    (e: React.ClipboardEvent) => {
      const items = e.clipboardData?.items;
      if (!items) return;
      const imageFiles: File[] = [];
      for (let i = 0; i < items.length; i++) {
        const item = items[i];
        if (item.type.startsWith('image/')) {
          const file = item.getAsFile();
          if (file) imageFiles.push(file);
        }
      }
      if (imageFiles.length > 0) {
        e.preventDefault();
        void handleFilesPicked(imageFiles);
      }
    },
    [handleFilesPicked]
  );

  const removeAttachment = useCallback((attachmentId: string) => {
    setAttachments((prev) =>
      prev.filter((item) => item.attachment_id !== attachmentId)
    );
    setImagePreviews((prev) => {
      const url = prev[attachmentId];
      if (url) URL.revokeObjectURL(url);
      const next = { ...prev };
      delete next[attachmentId];
      return next;
    });
  }, []);

  const handleArchive = useCallback(async () => {
    if (archiving || messages.length === 0) return;
    setArchiving(true);
    try {
      await archiveChat(agentId);
      setMessages([]);
      setContextWarning(null);
      const list = await fetchChatArchives(agentId);
      setArchives(list);
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Archive failed');
    } finally {
      setArchiving(false);
    }
  }, [agentId, archiving, messages.length, onError]);

  const handleViewArchive = useCallback(
    async (archiveId: string) => {
      try {
        const data = await fetchChatArchive(agentId, archiveId);
        setViewingArchive({ id: data.archive_id, messages: data.messages });
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to load archive');
      }
    },
    [agentId, onError]
  );

  const loadArchives = useCallback(async () => {
    try {
      const list = await fetchChatArchives(agentId);
      setArchives(list);
    } catch {
      // ignore
    }
  }, [agentId]);

  useEffect(() => {
    loadArchives();
  }, [loadArchives]);

  const handleSend = useCallback(
    (e: React.FormEvent) => {
      e.preventDefault();
      const text = input.trim();
      const attachmentIds = attachments.map((a) => a.attachment_id);
      if ((!text && attachmentIds.length === 0) || sending || uploadingAttachments)
        return;
      setInput('');
      setSending(true);
      setThinkingStatus('Connecting...');
      const userMessage: BirthChatMessageItem = {
        role: 'user',
        content: renderUserMessage(text, attachments),
      };
      setMessages((prev) => [...prev, userMessage]);

      const outboundMessage = text || 'Please analyze the uploaded attachments.';
      const phaseLabels: Record<string, string> = {
        routing: 'Routing to provider...',
        thinking: 'Thinking...',
        executing_tool: 'Executing tool...',
        synthesizing: 'Synthesizing council...',
      };

      sendChatStream(agentId, outboundMessage, routerMode, attachmentIds, {
        onStatus: (phase) => {
          const label = phase.startsWith('executing_tool:')
            ? `Running ${phase.split(':')[1]}...`
            : phaseLabels[phase] || 'Processing...';
          setThinkingStatus(label);
        },
        onDone: (res) => {
          setThinkingStatus(null);
          setSending(false);
          const assistantContent = res.attachment_notice
            ? `${res.assistant_content}\n\n${res.attachment_notice}`
            : res.assistant_content;
          setMessages((prev) => [
            ...prev,
            { role: 'assistant', content: assistantContent },
          ]);
          setAttachments([]);
          setImagePreviews((prev) => {
            Object.values(prev).forEach((url) => URL.revokeObjectURL(url));
            return {};
          });
          if (res.tool_log?.length) {
            const entries: ActivityLogItem[] = res.tool_log.map((entry, index) => ({
              ...entry,
              id: `${Date.now()}-${index}-${entry.tool_name}`,
            }));
            setActivityLog((prev) => [...entries, ...prev].slice(0, 20));
          }
          if (res.context_warning) {
            setContextWarning(
              `Context usage: ${res.context_warning.usage_percent}% of ${res.context_warning.model} window. Consider archiving.`
            );
          } else {
            setContextWarning(null);
          }
          if (res.auto_archived) {
            setMessages([{ role: 'assistant', content: assistantContent }]);
            setContextWarning(
              `Conversation auto-archived (${res.auto_archived.message_count} messages) to preserve context quality. View it in Archives.`
            );
            void loadArchives();
          }
          onRoutingTelemetryChange?.(res.routing_telemetry ?? null);
        },
        onError: (msg) => {
          setThinkingStatus(null);
          setSending(false);
          onError(msg || 'Send failed');
          setMessages((prev) => prev.slice(0, -1));
        },
      });
    },
    [
      agentId,
      attachments,
      input,
      loadArchives,
      onError,
      onRoutingTelemetryChange,
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
  const mentorDisplayName = mentorName || 'you';

  const handleMentorNameSubmit = () => {
    const name = mentorDraft.trim();
    if (!name || !onSetMentorName) return;
    onSetMentorName(name);
    setShowMentorPrompt(false);
    setMentorDraft('');
  };

  return (
    <div className="operational-chat">
      {showMentorPrompt && !mentorName && onSetMentorName && (
        <div className="operational-chat-mentor-prompt">
          <span>What would you like to be called?</span>
          <input
            type="text"
            value={mentorDraft}
            onChange={(e) => setMentorDraft(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleMentorNameSubmit()}
            placeholder="Your name..."
            className="operational-chat-mentor-input"
          />
          <button
            type="button"
            className="button-secondary operational-chat-mentor-btn"
            onClick={handleMentorNameSubmit}
            disabled={!mentorDraft.trim()}
          >
            Set
          </button>
          <button
            type="button"
            className="operational-chat-mentor-dismiss"
            onClick={() => setShowMentorPrompt(false)}
          >
            dismiss
          </button>
        </div>
      )}
      {contextWarning && (
        <div className="operational-chat-context-warning">
          {contextWarning}
          <button
            type="button"
            className="operational-chat-context-dismiss"
            onClick={() => setContextWarning(null)}
          >
            dismiss
          </button>
        </div>
      )}
      <div className="operational-chat-toolbar">
        <button
          type="button"
          className="button-secondary operational-chat-archive-btn"
          onClick={handleArchive}
          disabled={archiving || messages.length === 0}
        >
          {archiving ? 'Archiving...' : 'Archive'}
        </button>
        <button
          type="button"
          className="button-secondary operational-chat-archive-btn"
          onClick={() => setArchivesOpen(!archivesOpen)}
        >
          Archives{archives.length > 0 ? ` (${archives.length})` : ''}
        </button>
      </div>
      {archivesOpen && (
        <div className="operational-chat-archives-panel">
          {archives.length === 0 ? (
            <p className="operational-chat-archives-empty">No archived conversations.</p>
          ) : (
            <div className="operational-chat-archives-list">
              {archives.map((a) => (
                <button
                  key={a.archive_id}
                  type="button"
                  className="operational-chat-archive-item"
                  onClick={() => handleViewArchive(a.archive_id)}
                >
                  <span className="operational-chat-archive-date">{a.timestamp}</span>
                  <span className="operational-chat-archive-count">{a.message_count} msgs</span>
                  <span className="operational-chat-archive-preview">{a.preview}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      )}
      {viewingArchive && (
        <div className="operational-chat-archive-viewer">
          <div className="operational-chat-archive-viewer-header">
            <span>Archive: {viewingArchive.id}</span>
            <button
              type="button"
              className="button-secondary"
              onClick={() => setViewingArchive(null)}
            >
              Close
            </button>
          </div>
          <div className="operational-chat-messages">
            {viewingArchive.messages.map((m, i) => (
              <div
                key={i}
                className={`operational-chat-message operational-chat-message-${m.role}`}
              >
                <span className="operational-chat-message-role">
                  {m.role === 'assistant' ? (agentName || 'assistant') : mentorDisplayName}
                </span>
                <div className="operational-chat-message-content">{m.content}</div>
              </div>
            ))}
          </div>
        </div>
      )}
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
              {m.role === 'assistant' ? (agentName || 'assistant') : mentorDisplayName}
            </span>
            <div className="operational-chat-message-content">{m.content}</div>
          </div>
        ))}
        {thinkingStatus && (
          <div className="operational-chat-thinking">
            <span className="operational-chat-thinking-dot" />
            <span className="operational-chat-thinking-text">{thinkingStatus}</span>
          </div>
        )}
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
        onPaste={handlePaste}
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
                {imagePreviews[item.attachment_id] && (
                  <img
                    src={imagePreviews[item.attachment_id]}
                    alt={item.file_name}
                    className="operational-chat-attachment-thumb"
                  />
                )}
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
