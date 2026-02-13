import { useCallback, useEffect, useRef, useState } from 'react';
import {
  startAgenticRun,
  subscribeToAgenticStream,
  respondToAgent,
  confirmAgentTool,
  cancelAgenticRun,
} from '../api';
import './AgenticPanel.css';

interface AgenticPanelProps {
  agentId: string;
  agentName?: string;
  onError: (message: string) => void;
}

interface TimelineStep {
  id: number;
  type: string;
  turn: number;
  content: string;
  toolName?: string;
  success?: boolean;
  collapsed: boolean;
}

type RunStatus =
  | 'idle'
  | 'running'
  | 'waiting_for_mentor'
  | 'waiting_for_confirmation'
  | 'completed'
  | 'failed'
  | 'cancelled';

export default function AgenticPanel({
  agentId,
  agentName,
  onError,
}: AgenticPanelProps) {
  const [goal, setGoal] = useState('');
  const [autoApprove, setAutoApprove] = useState(false);
  const [maxTurns, setMaxTurns] = useState(15);
  const [status, setStatus] = useState<RunStatus>('idle');
  const [taskId, setTaskId] = useState<string | null>(null);
  const [turn, setTurn] = useState(0);
  const [steps, setSteps] = useState<TimelineStep[]>([]);
  const [mentorInput, setMentorInput] = useState('');
  const [starting, setStarting] = useState(false);
  const timelineEndRef = useRef<HTMLDivElement>(null);
  const eventSourceRef = useRef<EventSource | null>(null);
  const stepIdRef = useRef(0);

  // Scroll to bottom when new steps arrive
  useEffect(() => {
    timelineEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [steps]);

  // Cleanup EventSource on unmount
  useEffect(() => {
    return () => {
      eventSourceRef.current?.close();
    };
  }, []);

  const addStep = useCallback(
    (type: string, turnNum: number, content: string, extra?: Partial<TimelineStep>) => {
      const id = ++stepIdRef.current;
      setSteps((prev) => [
        ...prev,
        {
          id,
          type,
          turn: turnNum,
          content,
          collapsed: type === 'thinking' || type === 'tool_result',
          ...extra,
        },
      ]);
    },
    []
  );

  const handleStart = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      const text = goal.trim();
      if (!text || starting) return;

      setStarting(true);
      setSteps([]);
      setTurn(0);
      stepIdRef.current = 0;

      try {
        const result = await startAgenticRun(agentId, {
          goal: text,
          max_turns: maxTurns,
          auto_approve_safe_tools: autoApprove,
        });
        setTaskId(result.task_id);
        setStatus('running');

        // Subscribe to SSE stream
        const es = subscribeToAgenticStream(
          agentId,
          result.task_id,
          (eventName, rawData) => {
            // Backend sends {event, data: {…}} via serde tagged enum; unwrap nested payload
            const envelope = rawData as Record<string, unknown>;
            const data = (envelope.data ?? envelope) as Record<string, unknown>;
            switch (eventName) {
              case 'thinking':
                setTurn(data.turn as number);
                addStep('thinking', data.turn as number, data.content as string);
                break;
              case 'tool_call':
                addStep('tool_call', data.turn as number, JSON.stringify(data.arguments, null, 2), {
                  toolName: data.tool_name as string,
                });
                break;
              case 'tool_result':
                addStep('tool_result', data.turn as number, data.output as string, {
                  toolName: data.tool_name as string,
                  success: data.success as boolean,
                });
                break;
              case 'mentor_needed':
                setStatus('waiting_for_mentor');
                setTurn(data.turn as number);
                addStep('mentor_needed', data.turn as number, data.question as string);
                break;
              case 'confirmation_needed':
                setStatus('waiting_for_confirmation');
                setTurn(data.turn as number);
                addStep(
                  'confirmation_needed',
                  data.turn as number,
                  `Tool: ${data.tool_name}\nArguments: ${JSON.stringify(data.arguments, null, 2)}`,
                  { toolName: data.tool_name as string }
                );
                break;
              case 'done':
                setStatus((data.status as string) === 'cancelled' ? 'cancelled' : 'completed');
                addStep('done', data.turns_used as number, data.summary as string);
                es.close();
                break;
              case 'error':
                setStatus('failed');
                addStep('error', 0, data.message as string);
                es.close();
                break;
            }
          },
          () => {
            // SSE connection error — could be normal close or network issue
          }
        );
        eventSourceRef.current = es;
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to start agentic run');
        setStatus('idle');
      } finally {
        setStarting(false);
      }
    },
    [agentId, goal, maxTurns, autoApprove, starting, onError, addStep]
  );

  const handleMentorRespond = useCallback(async () => {
    if (!taskId || !mentorInput.trim()) return;
    try {
      await respondToAgent(agentId, taskId, mentorInput.trim());
      setMentorInput('');
      setStatus('running');
      addStep('mentor_response', turn, mentorInput.trim());
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Failed to send response');
    }
  }, [agentId, taskId, mentorInput, turn, onError, addStep]);

  const handleConfirm = useCallback(
    async (approved: boolean) => {
      if (!taskId) return;
      try {
        await confirmAgentTool(agentId, taskId, approved);
        setStatus('running');
        addStep('confirmation_response', turn, approved ? 'Approved' : 'Denied');
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to confirm');
      }
    },
    [agentId, taskId, turn, onError, addStep]
  );

  const handleCancel = useCallback(async () => {
    if (!taskId) return;
    try {
      await cancelAgenticRun(agentId, taskId);
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Failed to cancel');
    }
  }, [agentId, taskId, onError]);

  const toggleCollapse = useCallback((stepId: number) => {
    setSteps((prev) =>
      prev.map((s) => (s.id === stepId ? { ...s, collapsed: !s.collapsed } : s))
    );
  }, []);

  const isActive =
    status === 'running' ||
    status === 'waiting_for_mentor' ||
    status === 'waiting_for_confirmation';

  return (
    <div className="agentic-panel">
      {status === 'idle' ? (
        <form onSubmit={handleStart} className="agentic-goal-form">
          <textarea
            value={goal}
            onChange={(e) => setGoal(e.target.value)}
            placeholder={`Give ${agentName || 'your agent'} a task to work on autonomously...`}
            className="agentic-goal-input"
            disabled={starting}
          />
          <div className="agentic-goal-actions">
            <button
              type="submit"
              className="button-primary"
              disabled={starting || !goal.trim()}
            >
              {starting ? 'Starting...' : 'Run Task'}
            </button>
            <label>
              <input
                type="number"
                value={maxTurns}
                onChange={(e) => setMaxTurns(Math.min(50, Math.max(1, Number(e.target.value))))}
                style={{ width: '3rem' }}
                min={1}
                max={50}
              />
              max turns
            </label>
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
              <span className={`agentic-status-badge agentic-status-${status}`}>
                {status.replace(/_/g, ' ')}
              </span>
              <span className="agentic-turn-counter">
                Turn {turn}/{maxTurns}
              </span>
            </div>
            <div style={{ display: 'flex', gap: '0.5rem' }}>
              {isActive && (
                <button className="button-secondary" onClick={handleCancel}>
                  Cancel
                </button>
              )}
              {!isActive && (
                <button
                  className="button-primary"
                  onClick={() => {
                    setStatus('idle');
                    setTaskId(null);
                    setSteps([]);
                    setGoal('');
                    eventSourceRef.current?.close();
                  }}
                >
                  New Task
                </button>
              )}
            </div>
          </div>

          <div className="agentic-timeline">
            {steps.map((step) => (
              <div
                key={step.id}
                className={`agentic-step agentic-step-${step.type}${step.success === false ? ' agentic-step-error' : ''}`}
              >
                <div className="agentic-step-label">
                  <span>
                    {step.toolName ? `${step.type}: ${step.toolName}` : step.type.replace(/_/g, ' ')}
                  </span>
                  <span>Turn {step.turn}</span>
                  {step.collapsed && step.content.length > 100 && (
                    <button
                      className="agentic-step-toggle"
                      onClick={() => toggleCollapse(step.id)}
                    >
                      expand
                    </button>
                  )}
                  {!step.collapsed &&
                    (step.type === 'thinking' || step.type === 'tool_result') && (
                      <button
                        className="agentic-step-toggle"
                        onClick={() => toggleCollapse(step.id)}
                      >
                        collapse
                      </button>
                    )}
                </div>
                <div
                  className={`agentic-step-content${step.collapsed ? ' collapsed' : ''}`}
                  onClick={() => step.collapsed && toggleCollapse(step.id)}
                >
                  {step.content}
                </div>

                {step.type === 'mentor_needed' &&
                  status === 'waiting_for_mentor' &&
                  step.id === steps[steps.length - 1]?.id && (
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

                {step.type === 'confirmation_needed' &&
                  status === 'waiting_for_confirmation' &&
                  step.id === steps[steps.length - 1]?.id && (
                    <div className="agentic-confirm-actions">
                      <button
                        className="button-approve"
                        onClick={() => handleConfirm(true)}
                      >
                        Approve
                      </button>
                      <button
                        className="button-deny"
                        onClick={() => handleConfirm(false)}
                      >
                        Deny
                      </button>
                    </div>
                  )}
              </div>
            ))}
            <div ref={timelineEndRef} />
          </div>
        </>
      )}
    </div>
  );
}
