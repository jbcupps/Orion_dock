import { useCallback, useEffect, useRef, useState } from 'react';
import {
  fetchAgenticRuns,
  fetchAgenticTaskStatus,
  type AgenticRunInfo,
  type AgenticTaskStatusResponse,
} from '../api';
import './JobsTable.css';

interface JobsTableProps {
  agentId: string;
}

function formatTime(iso: string): string {
  if (!iso) return '';
  try {
    const d = new Date(iso);
    return d.toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  } catch {
    return iso;
  }
}

function badgeClass(status: string): string {
  switch (status) {
    case 'running':
      return 'jobs-badge jobs-badge-running';
    case 'completed':
    case 'success':
      return 'jobs-badge jobs-badge-completed';
    case 'failed':
      return 'jobs-badge jobs-badge-failed';
    case 'cancelled':
      return 'jobs-badge jobs-badge-cancelled';
    case 'partial':
      return 'jobs-badge jobs-badge-partial';
    default:
      return 'jobs-badge';
  }
}

function stepTypeLabel(stepType: string): string {
  switch (stepType) {
    case 'thinking':
      return 'Thinking';
    case 'tool_result':
      return 'Tool Result';
    case 'tool_error':
      return 'Tool Error';
    case 'done':
      return 'Done';
    case 'error':
      return 'Error';
    case 'ask_mentor':
      return 'Ask Mentor';
    case 'mentor_response':
      return 'Mentor Response';
    case 'confirmation_needed':
      return 'Confirmation';
    case 'register_mcp_skill':
      return 'Register Skill';
    case 'register_email_account':
      return 'Register Email';
    case 'manage_job':
      return 'Manage Job';
    case 'manage_proxy':
      return 'Manage Proxy';
    default:
      return stepType.replace(/_/g, ' ');
  }
}

function stepTypeClass(stepType: string): string {
  switch (stepType) {
    case 'thinking':
      return 'jobs-step-thinking';
    case 'tool_result':
      return 'jobs-step-tool-ok';
    case 'tool_error':
    case 'error':
      return 'jobs-step-error';
    case 'done':
      return 'jobs-step-done';
    case 'ask_mentor':
    case 'confirmation_needed':
      return 'jobs-step-mentor';
    default:
      return '';
  }
}

export default function JobsTable({ agentId }: JobsTableProps) {
  const [runs, setRuns] = useState<AgenticRunInfo[]>([]);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [expandedSteps, setExpandedSteps] = useState<AgenticTaskStatusResponse | null>(null);
  const [stepsLoading, setStepsLoading] = useState(false);
  const [stepsError, setStepsError] = useState<string | null>(null);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const stepsTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await fetchAgenticRuns(agentId);
      setRuns(data);
    } catch {
      // non-critical
    }
  }, [agentId]);

  useEffect(() => {
    load();
  }, [load]);

  // Auto-refresh: 5s if any active task, 30s otherwise
  useEffect(() => {
    const hasActive = runs.some((r) => r.status === 'running');
    const interval = hasActive ? 5000 : 30000;

    if (intervalRef.current) clearInterval(intervalRef.current);
    intervalRef.current = setInterval(load, interval);

    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, [runs, load]);

  // Poll steps for the expanded running task
  useEffect(() => {
    if (stepsTimerRef.current) {
      clearInterval(stepsTimerRef.current);
      stepsTimerRef.current = null;
    }
    if (!expandedId) {
      setExpandedSteps(null);
      setStepsError(null);
      return;
    }

    const run = runs.find((r) => r.task_id === expandedId);
    if (!run) return;

    let cancelled = false;

    const fetchSteps = async () => {
      try {
        const data = await fetchAgenticTaskStatus(agentId, expandedId);
        if (!cancelled) {
          setExpandedSteps(data);
          setStepsLoading(false);
          setStepsError(null);
        }
      } catch {
        if (!cancelled) {
          // Task may not be in memory (historical run from disk)
          setExpandedSteps(null);
          setStepsLoading(false);
          // Only show error if it's a running task that should be in memory
          if (run.status === 'running') {
            setStepsError('Unable to fetch task details.');
          }
        }
      }
    };

    setStepsLoading(true);
    fetchSteps();

    // Continue polling if the task is still active
    const isActive = run.status === 'running';
    if (isActive) {
      stepsTimerRef.current = setInterval(fetchSteps, 3000);
    }

    return () => {
      cancelled = true;
      if (stepsTimerRef.current) {
        clearInterval(stepsTimerRef.current);
        stepsTimerRef.current = null;
      }
    };
  }, [agentId, expandedId, runs]);

  const toggleExpand = (id: string) => {
    setExpandedId((prev) => (prev === id ? null : id));
  };

  return (
    <div className="jobs-panel">
      <h2>Jobs</h2>
      {runs.length === 0 ? (
        <p className="jobs-empty">No agentic runs yet.</p>
      ) : (
        <table className="jobs-table">
          <thead>
            <tr>
              <th>Goal</th>
              <th>Status</th>
              <th>Started</th>
              <th>Completed</th>
              <th>Turns</th>
              <th>Tools</th>
            </tr>
          </thead>
          <tbody>
            {runs.map((run) => (
              <>
                <tr
                  key={run.task_id}
                  className={`jobs-row${expandedId === run.task_id ? ' jobs-row-expanded' : ''}`}
                  onClick={() => toggleExpand(run.task_id)}
                >
                  <td className="jobs-goal" title={run.goal}>
                    {run.goal}
                  </td>
                  <td>
                    <span className={badgeClass(run.status)}>
                      {run.status}
                    </span>
                  </td>
                  <td>{formatTime(run.started_at)}</td>
                  <td>{run.completed_at ? formatTime(run.completed_at) : ''}</td>
                  <td>{run.turns}</td>
                  <td>{run.tool_calls}</td>
                </tr>
                {expandedId === run.task_id && (
                  <tr key={`${run.task_id}-detail`}>
                    <td colSpan={6} className="jobs-detail-cell">
                      <div className="jobs-detail">
                        <div className="jobs-detail-goal">{run.goal}</div>

                        {stepsLoading && (
                          <p className="jobs-detail-loading">Loading task details...</p>
                        )}

                        {stepsError && (
                          <p className="jobs-detail-error">{stepsError}</p>
                        )}

                        {expandedSteps && expandedSteps.steps.length > 0 && (
                          <div className="jobs-steps">
                            {expandedSteps.steps.map((step, i) => (
                              <div
                                key={i}
                                className={`jobs-step ${stepTypeClass(step.step_type)}`}
                              >
                                <div className="jobs-step-header">
                                  <span className="jobs-step-type">
                                    {stepTypeLabel(step.step_type)}
                                  </span>
                                  <span className="jobs-step-turn">Turn {step.turn}</span>
                                  <span className="jobs-step-time">
                                    {formatTime(step.timestamp)}
                                  </span>
                                </div>
                                <div className="jobs-step-content">
                                  {step.content.length > 500
                                    ? step.content.slice(0, 500) + '...'
                                    : step.content}
                                </div>
                              </div>
                            ))}
                          </div>
                        )}

                        {!stepsLoading && !expandedSteps && run.summary && (
                          <div className="jobs-detail-summary">
                            <strong>Summary:</strong> {run.summary}
                          </div>
                        )}

                        {expandedSteps && expandedSteps.steps.length === 0 && !stepsLoading && (
                          <p className="jobs-detail-empty">No steps recorded yet.</p>
                        )}
                      </div>
                    </td>
                  </tr>
                )}
              </>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
