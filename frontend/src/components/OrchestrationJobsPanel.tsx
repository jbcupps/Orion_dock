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
  const [name, setName] = useState('');
  const [cron, setCron] = useState(DEFAULT_CRON);
  const [mode, setMode] = useState<OrchestrationJobMode>('id_check');
  const [goalTemplate, setGoalTemplate] = useState('');
  const [enabled, setEnabled] = useState(true);
  const [escalateMedium, setEscalateMedium] = useState(false);
  const [flagHighToMentor, setFlagHighToMentor] = useState(true);

  const resetForm = useCallback(() => {
    setEditingJobId(null);
    setName('');
    setCron(DEFAULT_CRON);
    setMode('id_check');
    setGoalTemplate('');
    setEnabled(true);
    setEscalateMedium(false);
    setFlagHighToMentor(true);
  }, []);

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
      if (!name.trim() || !goalTemplate.trim() || !cron.trim()) return;
      setSaving(true);
      try {
        if (editingJobId) {
          await updateOrchestrationJob(agentId, editingJobId, {
            name: name.trim(),
            cron: cron.trim(),
            mode,
            goal_template: goalTemplate.trim(),
            enabled,
            significance_policy: {
              escalate_medium: escalateMedium,
              flag_high_to_mentor: flagHighToMentor,
            },
          });
        } else {
          await createOrchestrationJob(agentId, {
            name: name.trim(),
            cron: cron.trim(),
            mode,
            goal_template: goalTemplate.trim(),
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
