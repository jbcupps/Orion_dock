import { useEffect, useState } from 'react';
import { fetchAgenticTaskStatus, type AgenticTaskStatusResponse } from '../api';
import './InlineTaskMonitor.css';

interface InlineTaskMonitorProps {
  agentId: string;
  taskId: string;
  goal: string;
  onViewTimeline?: () => void;
}

const TERMINAL_STATUSES = new Set(['completed', 'failed', 'cancelled']);

export default function InlineTaskMonitor({
  agentId,
  taskId,
  goal,
  onViewTimeline,
}: InlineTaskMonitorProps) {
  const [expanded, setExpanded] = useState(false);
  const [status, setStatus] = useState<AgenticTaskStatusResponse['status']>('running');
  const [turn, setTurn] = useState(0);
  const [steps, setSteps] = useState<AgenticTaskStatusResponse['steps']>([]);
  const [summary, setSummary] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let timer: number | null = null;

    const poll = async () => {
      try {
        const data = await fetchAgenticTaskStatus(agentId, taskId);
        if (cancelled) return;
        setStatus(data.status);
        setTurn(data.turn);
        setSteps(data.steps);

        const summaryStep = [...data.steps]
          .reverse()
          .find((s) => s.step_type === 'done' || s.step_type === 'error');
        if (summaryStep) setSummary(summaryStep.content.trim());

        if (TERMINAL_STATUSES.has(data.status)) return;
      } catch {
        if (cancelled) return;
      }
      if (!cancelled) {
        timer = window.setTimeout(poll, 3000);
      }
    };

    void poll();
    return () => {
      cancelled = true;
      if (timer !== null) clearTimeout(timer);
    };
  }, [agentId, taskId]);

  const toolCalls = steps.filter(
    (s) => s.step_type === 'tool_result' || s.step_type === 'tool_error'
  ).length;
  const isTerminal = TERMINAL_STATUSES.has(status);
  const statusLabel = status.replace(/_/g, ' ');

  return (
    <div className={`task-monitor${isTerminal ? ' task-monitor-terminal' : ''}`}>
      <button
        type="button"
        className="task-monitor-header"
        onClick={() => setExpanded(!expanded)}
        aria-label={expanded ? 'Collapse task details' : 'Expand task details'}
        aria-expanded={expanded}
      >
        <span className={`task-monitor-badge task-monitor-badge-${isTerminal ? status : 'running'}`}>
          {statusLabel}
        </span>
        <span className="task-monitor-stats">
          Turn {turn} &middot; {toolCalls} tool{toolCalls !== 1 ? 's' : ''}
        </span>
        <span className="task-monitor-chevron">{expanded ? '\u25BC' : '\u25B6'}</span>
      </button>

      {summary && !expanded && (
        <div className="task-monitor-summary">{summary}</div>
      )}

      {expanded && (
        <div className="task-monitor-details">
          <div className="task-monitor-goal">{goal}</div>
          {steps.length === 0 && (
            <p className="task-monitor-empty">No steps recorded yet.</p>
          )}
          <div className="task-monitor-steps">
            {steps.map((step, i) => (
              <div
                key={i}
                className={`task-monitor-step task-monitor-step-${step.step_type}`}
              >
                <div className="task-monitor-step-header">
                  <span className="task-monitor-step-type">{step.step_type}</span>
                  <span className="task-monitor-step-turn">T{step.turn}</span>
                </div>
                <div className="task-monitor-step-content">
                  {step.content.length > 300
                    ? step.content.slice(0, 300) + '...'
                    : step.content}
                </div>
              </div>
            ))}
          </div>
          {onViewTimeline && (
            <button
              type="button"
              className="button-secondary task-monitor-timeline-btn"
              onClick={(e) => {
                e.stopPropagation();
                onViewTimeline();
              }}
            >
              View full timeline
            </button>
          )}
        </div>
      )}
    </div>
  );
}
