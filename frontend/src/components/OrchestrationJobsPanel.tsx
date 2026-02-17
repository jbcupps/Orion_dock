import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  createOrchestrationJob,
  deleteOrchestrationJob,
  fetchOrchestrationJobs,
  fetchOrchestrationLogs,
  runOrchestrationJobNow,
  setOrchestrationJobEnabled,
  updateOrchestrationJob,
  type OrchestrationJob,
  type OrchestrationJobLogEntry,
  type OrchestrationJobMode,
} from '../api';
import './OrchestrationJobsPanel.css';

interface OrchestrationJobsPanelProps {
  agentId: string;
  onError: (message: string) => void;
}

const DEFAULT_CRON = '0 */15 * * * * *';

function parseTimeToken(raw: string): { hour: number; minute: number } | null {
  const token = raw.trim().toLowerCase().replace(/[.,;!?]+$/g, '');
  if (!token) return null;
  const hasAm = token.endsWith('am');
  const hasPm = token.endsWith('pm');
  const core = hasAm || hasPm ? token.slice(0, -2).trim() : token;
  if (!core) return null;
  const [hPart, mPart] = core.includes(':') ? core.split(':') : [core, '0'];
  const h = Number(hPart);
  const m = Number(mPart);
  if (!Number.isFinite(h) || !Number.isFinite(m) || m < 0 || m > 59) return null;
  let hour = h;
  if (hasAm || hasPm) {
    if (hour < 1 || hour > 12) return null;
    if (hasPm && hour !== 12) hour += 12;
    if (hasAm && hour === 12) hour = 0;
  } else if (hour < 0 || hour > 23) {
    return null;
  }
  return { hour, minute: m };
}

function detectTime(text: string): { hour: number; minute: number } {
  const tokens = text
    .toLowerCase()
    .split(/\s+/)
    .map((t) => t.trim())
    .filter(Boolean);
  for (const token of tokens) {
    const parsed = parseTimeToken(token);
    if (parsed) return parsed;
  }
  return { hour: 6, minute: 0 };
}

function inferMode(text: string): OrchestrationJobMode {
  const lower = text.toLowerCase();
  if (
    lower.includes('id check') ||
    lower.includes('lightweight') ||
    lower.includes('monitor') ||
    lower.includes('watch')
  ) {
    return 'id_check';
  }
  return 'agentic_run';
}

function inferCron(text: string): string {
  const lower = text.toLowerCase();
  const { hour, minute } = detectTime(text);

  const everyMinutes = lower.match(/every\s+(\d+)\s+minutes?/);
  if (everyMinutes) {
    const n = Math.min(59, Math.max(1, Number(everyMinutes[1])));
    return `0 */${n} * * * * *`;
  }

  const everyHours = lower.match(/every\s+(\d+)\s+hours?/);
  if (everyHours) {
    const n = Math.min(23, Math.max(1, Number(everyHours[1])));
    return `0 0 */${n} * * * *`;
  }

  if (lower.includes('hourly') || lower.includes('every hour')) {
    return '0 0 * * * * *';
  }
  if (lower.includes('weekdays') || lower.includes('weekday')) {
    return `0 ${minute} ${hour} * * 1-5 *`;
  }
  const dayMap: Record<string, number> = {
    sunday: 0,
    monday: 1,
    tuesday: 2,
    wednesday: 3,
    thursday: 4,
    friday: 5,
    saturday: 6,
  };
  for (const [name, dow] of Object.entries(dayMap)) {
    if (lower.includes(name)) {
      return `0 ${minute} ${hour} * * ${dow} *`;
    }
  }
  if (lower.includes('weekly')) {
    return `0 ${minute} ${hour} * * 1 *`;
  }
  if (lower.includes('monthly')) {
    const dayMatch = lower.match(/day\s+(\d{1,2})/);
    const day = dayMatch ? Math.min(28, Math.max(1, Number(dayMatch[1]))) : 1;
    return `0 ${minute} ${hour} ${day} * * *`;
  }
  return `0 ${minute} ${hour} * * * *`;
}

function inferName(text: string): string {
  const trimmed = text.trim();
  if (!trimmed) return 'Scheduled Task';
  const forMatch = trimmed.match(/\bfor\b(.+)$/i);
  if (forMatch?.[1]) {
    const words = forMatch[1].trim().split(/\s+/).slice(0, 4).join(' ');
    if (words) return `Job: ${words}`;
  }
  return trimmed.split(/\s+/).slice(0, 4).join(' ');
}

function formatTime(iso?: string): string {
  if (!iso) return '—';
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

export default function OrchestrationJobsPanel({
  agentId,
  onError,
}: OrchestrationJobsPanelProps) {
  const [jobs, setJobs] = useState<OrchestrationJob[]>([]);
  const [logs, setLogs] = useState<OrchestrationJobLogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [runningJobId, setRunningJobId] = useState<string | null>(null);

  const [editingJobId, setEditingJobId] = useState<string | null>(null);
  const [jobDescription, setJobDescription] = useState('');
  const [name, setName] = useState('');
  const [cron, setCron] = useState(DEFAULT_CRON);
  const [mode, setMode] = useState<OrchestrationJobMode>('id_check');
  const [goalTemplate, setGoalTemplate] = useState('');
  const [enabled, setEnabled] = useState(true);
  const [escalateMedium, setEscalateMedium] = useState(false);
  const [flagHighToMentor, setFlagHighToMentor] = useState(true);

  const resetForm = useCallback(() => {
    setEditingJobId(null);
    setJobDescription('');
    setName('');
    setCron(DEFAULT_CRON);
    setMode('id_check');
    setGoalTemplate('');
    setEnabled(true);
    setEscalateMedium(false);
    setFlagHighToMentor(true);
  }, []);

  const applyDescription = useCallback(() => {
    const text = jobDescription.trim();
    if (!text) return;
    setName((prev) => (prev.trim() ? prev : inferName(text)));
    setMode(inferMode(text));
    setCron(inferCron(text));
    setGoalTemplate(text);
  }, [jobDescription]);

  const load = useCallback(async () => {
    try {
      const [jobsRes, logsRes] = await Promise.all([
        fetchOrchestrationJobs(agentId),
        fetchOrchestrationLogs(agentId, 20),
      ]);
      setJobs(jobsRes.jobs);
      setLogs(logsRes.logs);
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Failed to load orchestration jobs');
    } finally {
      setLoading(false);
    }
  }, [agentId, onError]);

  useEffect(() => {
    setLoading(true);
    load();
    const interval = setInterval(load, 15000);
    return () => clearInterval(interval);
  }, [load]);

  const sortedJobs = useMemo(
    () => [...jobs].sort((a, b) => a.name.localeCompare(b.name)),
    [jobs]
  );

  const handleSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      const hasDescription = jobDescription.trim().length > 0;
      const resolvedName = name.trim() || (hasDescription ? inferName(jobDescription) : '');
      const resolvedGoal = goalTemplate.trim() || (hasDescription ? jobDescription.trim() : '');
      const resolvedCron = cron.trim() || (hasDescription ? inferCron(jobDescription) : '');
      const resolvedMode = hasDescription ? inferMode(jobDescription) : mode;
      if (!resolvedName || !resolvedGoal || !resolvedCron) return;
      setSaving(true);
      try {
        if (editingJobId) {
          await updateOrchestrationJob(agentId, editingJobId, {
            name: resolvedName,
            cron: resolvedCron,
            mode: resolvedMode,
            goal_template: resolvedGoal,
            enabled,
            significance_policy: {
              escalate_medium: escalateMedium,
              flag_high_to_mentor: flagHighToMentor,
            },
          });
        } else {
          await createOrchestrationJob(agentId, {
            name: resolvedName,
            cron: resolvedCron,
            mode: resolvedMode,
            goal_template: resolvedGoal,
            enabled,
            significance_policy: {
              escalate_medium: escalateMedium,
              flag_high_to_mentor: flagHighToMentor,
            },
          });
        }
        await load();
        resetForm();
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to save orchestration job');
      } finally {
        setSaving(false);
      }
    },
    [
      agentId,
      cron,
      editingJobId,
      enabled,
      escalateMedium,
      flagHighToMentor,
      goalTemplate,
      jobDescription,
      load,
      mode,
      name,
      onError,
      resetForm,
    ]
  );

  const handleEdit = useCallback((job: OrchestrationJob) => {
    setEditingJobId(job.job_id);
    setName(job.name);
    setCron(job.cron);
    setMode(job.mode);
    setGoalTemplate(job.goal_template);
    setEnabled(job.enabled);
    setEscalateMedium(Boolean(job.significance_policy?.escalate_medium));
    setFlagHighToMentor(
      job.significance_policy?.flag_high_to_mentor === undefined
        ? true
        : Boolean(job.significance_policy.flag_high_to_mentor)
    );
  }, []);

  const handleToggleEnabled = useCallback(
    async (job: OrchestrationJob) => {
      try {
        await setOrchestrationJobEnabled(agentId, job.job_id, !job.enabled);
        await load();
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to toggle job');
      }
    },
    [agentId, load, onError]
  );

  const handleRunNow = useCallback(
    async (job: OrchestrationJob) => {
      setRunningJobId(job.job_id);
      try {
        await runOrchestrationJobNow(agentId, job.job_id);
        await load();
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to run job');
      } finally {
        setRunningJobId(null);
      }
    },
    [agentId, load, onError]
  );

  const handleDelete = useCallback(
    async (job: OrchestrationJob) => {
      if (!window.confirm(`Delete job "${job.name}"?`)) return;
      try {
        await deleteOrchestrationJob(agentId, job.job_id);
        if (editingJobId === job.job_id) resetForm();
        await load();
      } catch (e) {
        onError(e instanceof Error ? e.message : 'Failed to delete job');
      }
    },
    [agentId, editingJobId, load, onError, resetForm]
  );

  if (loading) {
    return (
      <div className="orchestration-panel">
        <h2>Orchestration</h2>
        <p className="orchestration-muted">Loading scheduled jobs…</p>
      </div>
    );
  }

  return (
    <div className="orchestration-panel">
      <h2>Orchestration</h2>
      <p className="orchestration-muted">
        UTC cron schedules. Id runs lightweight checks; high-significance findings can escalate.
      </p>

      <form className="orchestration-form" onSubmit={handleSubmit}>
        <input
          value={jobDescription}
          onChange={(e) => setJobDescription(e.target.value)}
          placeholder="Describe the job (e.g., Check Cincinnati weather every day at 6:00 AM)"
          className="orchestration-input orchestration-input-wide"
          disabled={saving}
        />
        <button
          type="button"
          className="button-secondary orchestration-suggest"
          onClick={applyDescription}
          disabled={saving || !jobDescription.trim()}
        >
          Use suggestion
        </button>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Job name"
          className="orchestration-input"
          disabled={saving}
        />
        <input
          value={cron}
          onChange={(e) => setCron(e.target.value)}
          placeholder="Cron (UTC)"
          className="orchestration-input"
          disabled={saving}
        />
        <select
          value={mode}
          onChange={(e) => setMode(e.target.value as OrchestrationJobMode)}
          className="orchestration-input"
          disabled={saving}
        >
          <option value="id_check">Id check</option>
          <option value="agentic_run">Agentic run</option>
        </select>
        <textarea
          value={goalTemplate}
          onChange={(e) => setGoalTemplate(e.target.value)}
          placeholder="Goal / check prompt"
          className="orchestration-textarea"
          disabled={saving}
        />
        <label className="orchestration-toggle">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
            disabled={saving}
          />
          enabled
        </label>
        <label className="orchestration-toggle">
          <input
            type="checkbox"
            checked={escalateMedium}
            onChange={(e) => setEscalateMedium(e.target.checked)}
            disabled={saving}
          />
          escalate medium
        </label>
        <label className="orchestration-toggle">
          <input
            type="checkbox"
            checked={flagHighToMentor}
            onChange={(e) => setFlagHighToMentor(e.target.checked)}
            disabled={saving}
          />
          flag high to mentor
        </label>
        <div className="orchestration-actions">
          <button className="button-primary" type="submit" disabled={saving}>
            {saving ? 'Saving…' : editingJobId ? 'Update job' : 'Create job'}
          </button>
          {editingJobId && (
            <button
              className="button-secondary"
              type="button"
              onClick={resetForm}
              disabled={saving}
            >
              Cancel edit
            </button>
          )}
        </div>
      </form>

      <div className="orchestration-jobs">
        {sortedJobs.length === 0 ? (
          <p className="orchestration-muted">No scheduled jobs yet.</p>
        ) : (
          <table className="orchestration-table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Mode</th>
                <th>Cron</th>
                <th>Status</th>
                <th>Next run</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {sortedJobs.map((job) => (
                <tr key={job.job_id}>
                  <td>{job.name}</td>
                  <td>{job.mode}</td>
                  <td className="orchestration-code">{job.cron}</td>
                  <td>
                    <span className={job.enabled ? 'orchestration-ok' : 'orchestration-muted'}>
                      {job.enabled ? 'enabled' : 'disabled'}
                    </span>
                    {job.last_status ? ` / ${job.last_status}` : ''}
                    {job.last_significance ? ` / ${job.last_significance}` : ''}
                  </td>
                  <td>{formatTime(job.next_run_at)}</td>
                  <td className="orchestration-row-actions">
                    <button
                      className="button-secondary"
                      type="button"
                      onClick={() => handleEdit(job)}
                    >
                      Edit
                    </button>
                    <button
                      className="button-secondary"
                      type="button"
                      onClick={() => handleToggleEnabled(job)}
                    >
                      {job.enabled ? 'Disable' : 'Enable'}
                    </button>
                    <button
                      className="button-secondary"
                      type="button"
                      onClick={() => handleRunNow(job)}
                      disabled={runningJobId === job.job_id}
                    >
                      {runningJobId === job.job_id ? 'Running…' : 'Run now'}
                    </button>
                    <button
                      className="button-secondary"
                      type="button"
                      onClick={() => handleDelete(job)}
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="orchestration-logs">
        <h3>Recent log entries</h3>
        {logs.length === 0 ? (
          <p className="orchestration-muted">No orchestration logs yet.</p>
        ) : (
          <div className="orchestration-log-list">
            {logs.map((log) => (
              <div key={log.entry_id} className="orchestration-log-item">
                <div className="orchestration-log-head">
                  <strong>{log.job_name}</strong>
                  <span>
                    {log.significance} / {log.decision} / {log.status}
                  </span>
                </div>
                <div className="orchestration-muted">
                  {formatTime(log.started_at)} {'->'} {formatTime(log.completed_at)}
                  {log.task_id ? ` / task ${log.task_id}` : ''}
                </div>
                <pre className="orchestration-log-summary">{log.summary}</pre>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
