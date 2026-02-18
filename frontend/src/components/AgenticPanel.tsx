import { useCallback, useEffect, useRef, useState } from 'react';
import { useAgenticStream } from '../hooks/useAgenticStream';
import './AgenticPanel.css';

interface AgenticPanelProps {
  agentId: string;
  agentName?: string;
  routerMode?: AgenticRouterMode;
  onRouterModeChange?: (mode: AgenticRouterMode) => void;
  onError: (message: string) => void;
  onBusyChange?: (busy: boolean) => void;
  externalTask?: { taskId: string; goal: string } | null;
  onExternalTaskConsumed?: () => void;
}

type AgenticRouterMode = 'auto' | 'think_hard' | 'think_harder';

const ROUTER_MAX_TURN_DEFAULTS: Record<AgenticRouterMode, number> = {
  auto: 15,
  think_hard: 24,
  think_harder: 36,
};

export default function AgenticPanel({
  agentId,
  agentName,
  routerMode: routerModeProp,
  onRouterModeChange,
  onError,
  onBusyChange,
  externalTask,
  onExternalTaskConsumed,
}: AgenticPanelProps) {
  const stream = useAgenticStream(agentId, onError);

  const [goal, setGoal] = useState('');
  const [autoApprove, setAutoApprove] = useState(false);
  const routerMode = routerModeProp ?? 'auto';
  const [maxTurns, setMaxTurns] = useState(ROUTER_MAX_TURN_DEFAULTS[routerMode]);
  const [mentorInput, setMentorInput] = useState('');
  const timelineEndRef = useRef<HTMLDivElement>(null);
  const activeTierLabel =
    routerMode === 'think_harder'
      ? 'Pro'
      : routerMode === 'think_hard'
        ? 'Standard'
        : 'Fast';

  // Scroll to bottom when new entries arrive
  useEffect(() => {
    timelineEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [stream.entries]);

  // Propagate busy state to parent
  useEffect(() => {
    onBusyChange?.(stream.isActive);
  }, [stream.isActive, onBusyChange]);

  // Sync maxTurns when parent changes routerMode
  useEffect(() => {
    setMaxTurns(ROUTER_MAX_TURN_DEFAULTS[routerMode]);
  }, [routerMode]);

  const handleRouterModeChange = useCallback((nextMode: AgenticRouterMode) => {
    onRouterModeChange?.(nextMode);
    setMaxTurns(ROUTER_MAX_TURN_DEFAULTS[nextMode]);
  }, [onRouterModeChange]);

  // Handle external task (launched from chat)
  useEffect(() => {
    if (!externalTask?.taskId) return;
    if (stream.taskId === externalTask.taskId && stream.status !== 'idle') return;
    setGoal(externalTask.goal || '');
    stream.reset();
    stream.attachToTask(externalTask.taskId);
    onExternalTaskConsumed?.();
  }, [
    stream,
    externalTask,
    onExternalTaskConsumed,
  ]);

  const handleStart = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      await stream.start(goal, { maxTurns, autoApprove, routerMode });
    },
    [goal, maxTurns, autoApprove, routerMode, stream],
  );

  const handleMentorRespond = useCallback(async () => {
    await stream.respondToMentor(mentorInput);
    setMentorInput('');
  }, [stream, mentorInput]);

  const toggleCollapse = useCallback((entryId: number) => {
    // Collapse state is UI-only; manage via local override map
    setCollapsed((prev) => ({ ...prev, [entryId]: !prev[entryId] }));
  }, []);
  const [collapsed, setCollapsed] = useState<Record<number, boolean>>({});

  const isCollapsed = useCallback(
    (entry: { id: number; type: string }) => {
      if (entry.id in collapsed) return collapsed[entry.id];
      return entry.type === 'thinking' || entry.type === 'tool_result';
    },
    [collapsed],
  );

  return (
    <div className="agentic-panel">
      {stream.status === 'idle' ? (
        <form onSubmit={handleStart} className="agentic-goal-form">
          <textarea
            value={goal}
            onChange={(e) => setGoal(e.target.value)}
            placeholder={`Give ${agentName || 'your agent'} a task to work on autonomously...`}
            className="agentic-goal-input"
            disabled={stream.starting}
          />
          <div className="agentic-goal-actions">
            <button
              type="submit"
              className="button-primary"
              disabled={stream.starting || !goal.trim()}
            >
              {stream.starting ? 'Starting...' : 'Run Task'}
            </button>
            <label>
              <input
                type="number"
                value={maxTurns}
                onChange={(e) =>
                  setMaxTurns(Math.min(50, Math.max(1, Number(e.target.value) || 1)))
                }
                style={{ width: '3rem' }}
                min={1}
                max={50}
              />
              max turns
            </label>
            <label>
              router
              <select
                value={routerMode}
                onChange={(e) =>
                  handleRouterModeChange(e.target.value as AgenticRouterMode)
                }
                className="agentic-router-select"
              >
                <option value="auto">Fast</option>
                <option value="think_hard">Standard</option>
                <option value="think_harder">Pro</option>
              </select>
            </label>
            <span className="agentic-tier-badge">Tier: {activeTierLabel}</span>
            <label>
              <input
                type="checkbox"
                checked={autoApprove}
                onChange={(e) => setAutoApprove(e.target.checked)}
              />
              auto-approve safe tools
            </label>
          </div>
        </form>
      ) : (
        <>
          <div className="agentic-header">
            <div>
              <span className={`agentic-status-badge agentic-status-${stream.status}`}>
                {stream.status.replace(/_/g, ' ')}
              </span>
              <span className="agentic-tier-badge">Tier: {activeTierLabel}</span>
              <span className="agentic-turn-counter">
                Turn {stream.turn}/{maxTurns}
              </span>
            </div>
            <div style={{ display: 'flex', gap: '0.5rem' }}>
              {stream.isActive && (
                <button className="button-secondary" onClick={stream.cancel}>
                  Cancel
                </button>
              )}
              {!stream.isActive && (
                <button
                  className="button-primary"
                  onClick={() => {
                    stream.reset();
                    setGoal('');
                  }}
                >
                  New Task
                </button>
              )}
            </div>
          </div>

          <div className="agentic-timeline">
            {stream.entries.map((entry) => {
              const entryCollapsed = isCollapsed(entry);
              return (
                <div
                  key={entry.id}
                  className={`agentic-step agentic-step-${entry.type}${entry.success === false ? ' agentic-step-error' : ''}`}
                >
                  <div className="agentic-step-label">
                    <span>
                      {entry.toolName ? `${entry.type}: ${entry.toolName}` : entry.type.replace(/_/g, ' ')}
                    </span>
                    <span>Turn {entry.turn}</span>
                    {entryCollapsed && entry.content.length > 100 && (
                      <button
                        className="agentic-step-toggle"
                        onClick={() => toggleCollapse(entry.id)}
                        aria-label="Expand step details"
                      >
                        expand
                      </button>
                    )}
                    {!entryCollapsed &&
                      (entry.type === 'thinking' || entry.type === 'tool_result') && (
                        <button
                          className="agentic-step-toggle"
                          onClick={() => toggleCollapse(entry.id)}
                          aria-label="Collapse step details"
                        >
                          collapse
                        </button>
                      )}
                  </div>
                  <div
                    className={`agentic-step-content${entryCollapsed ? ' collapsed' : ''}`}
                    onClick={() => entryCollapsed && toggleCollapse(entry.id)}
                  >
                    {entry.content}
                  </div>

                  {entry.type === 'mentor_needed' &&
                    stream.status === 'waiting_for_mentor' &&
                    entry.id === stream.entries[stream.entries.length - 1]?.id && (
                      <div className="agentic-mentor-form">
                        <input
                          type="text"
                          value={mentorInput}
                          onChange={(e) => setMentorInput(e.target.value)}
                          placeholder="Your response..."
                          className="agentic-mentor-input"
                          onKeyDown={(e) => {
                            if (e.key === 'Enter') handleMentorRespond();
                          }}
                        />
                        <button
                          className="button-primary"
                          onClick={handleMentorRespond}
                          disabled={!mentorInput.trim()}
                        >
                          Respond
                        </button>
                      </div>
                    )}

                  {entry.type === 'confirmation_needed' &&
                    stream.status === 'waiting_for_confirmation' &&
                    entry.id === stream.entries[stream.entries.length - 1]?.id && (
                      <div className="agentic-confirm-actions">
                        <button
                          className="button-approve"
                          onClick={() => stream.confirm(true)}
                        >
                          Approve
                        </button>
                        <button
                          className="button-deny"
                          onClick={() => stream.confirm(false)}
                        >
                          Deny
                        </button>
                      </div>
                    )}
                </div>
              );
            })}
            <div ref={timelineEndRef} />
          </div>
        </>
      )}
    </div>
  );
}
