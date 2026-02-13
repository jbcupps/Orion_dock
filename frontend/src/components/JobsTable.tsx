import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchAgenticRuns, type AgenticRunInfo } from '../api';
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

export default function JobsTable({ agentId }: JobsTableProps) {
  const [runs, setRuns] = useState<AgenticRunInfo[]>([]);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

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
                  className="jobs-row"
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
                {expandedId === run.task_id && run.summary && (
                  <tr key={`${run.task_id}-summary`}>
                    <td colSpan={6} className="jobs-summary">
                      {run.summary}
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
