import { useEffect, useState, useCallback } from 'react';
import {
  fetchSkills,
  fetchMissingSecrets,
  type SkillInfo,
  type MissingSkillSecret,
} from '../api';
import { useAgenticStream } from '../hooks/useAgenticStream';
import { useFocusTrap } from '../hooks/useFocusTrap';
import './MissionControl.css';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface MissionControlProps {
  agentId: string;
  agentName?: string;
  routerMode?: 'auto' | 'think_hard' | 'think_harder';
  onError: (message: string) => void;
  onBusyChange?: (busy: boolean) => void;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function MissionControl({
  agentId,
  agentName,
  routerMode = 'auto',
  onError,
  onBusyChange,
}: MissionControlProps) {
  const stream = useAgenticStream(agentId, onError);

  // --- Local UI state ---
  const [goalInput, setGoalInput] = useState('');
  const [currentGoal, setCurrentGoal] = useState<string | null>(null);
  const [mentorInput, setMentorInput] = useState('');
  const [showLog, setShowLog] = useState(false);
  const hasErrors = stream.entries.some(
    (e) => e.type === 'error' || (e.type === 'tool_result' && e.success === false)
  );

  // --- Skills ---
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [missingSecrets, setMissingSecrets] = useState<MissingSkillSecret[]>([]);
  const [showSkills, setShowSkills] = useState(false);

  // Focus traps for modal dialogs
  const mentorModalActive = stream.status === 'waiting_for_mentor' && !!stream.mentorQuestion;
  const confirmModalActive = stream.status === 'waiting_for_confirmation' && !!stream.confirmationInfo;
  const mentorTrapRef = useFocusTrap<HTMLDivElement>(mentorModalActive);
  const confirmTrapRef = useFocusTrap<HTMLDivElement>(confirmModalActive);

  // Propagate busy state
  useEffect(() => {
    onBusyChange?.(stream.isActive);
  }, [stream.isActive, onBusyChange]);

  // Auto-expand log on errors
  useEffect(() => {
    if (hasErrors) setShowLog(true);
  }, [hasErrors]);

  // Load skills on mount and periodically
  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const [s, m] = await Promise.all([
          fetchSkills(agentId),
          fetchMissingSecrets(agentId),
        ]);
        if (!cancelled) {
          setSkills(s);
          setMissingSecrets(m);
        }
      } catch {
        // Skills endpoint may not be available during birth
      }
    };
    load();
    const iv = setInterval(load, 30_000);
    return () => {
      cancelled = true;
      clearInterval(iv);
    };
  }, [agentId]);

  // --- Actions ---

  const tierLabel =
    routerMode === 'think_harder'
      ? 'Pro'
      : routerMode === 'think_hard'
        ? 'Standard'
        : 'Fast';

  const maxTurns =
    routerMode === 'think_harder' ? 36 : routerMode === 'think_hard' ? 24 : 15;

  const handleLaunch = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      const text = goalInput.trim();
      if (!text) return;
      setCurrentGoal(text);
      await stream.start(text, { maxTurns, autoApprove: false, routerMode });
    },
    [goalInput, maxTurns, routerMode, stream],
  );

  const handleMentorRespond = useCallback(async () => {
    await stream.respondToMentor(mentorInput);
    setMentorInput('');
  }, [stream, mentorInput]);

  const handleReset = useCallback(() => {
    stream.reset();
    setCurrentGoal(null);
    setGoalInput('');
  }, [stream]);

  // --- Skill status helper ---
  const skillStatus = useCallback(
    (skill: SkillInfo): 'ok' | 'missing' | 'partial' => {
      const missing = missingSecrets.filter((m) => m.skill_id === skill.id);
      if (missing.length === 0) return 'ok';
      if (missing.some((m) => m.required)) return 'missing';
      return 'partial';
    },
    [missingSecrets],
  );

  // --- Render ---

  return (
    <div className="mc">
      {/* ==================== FOCUS AREA ==================== */}
      <div className="mc-focus">
        {/* Goal bar */}
        {stream.status === 'idle' ? (
          <form onSubmit={handleLaunch} className="mc-goal-form">
            <input
              type="text"
              value={goalInput}
              onChange={(e) => setGoalInput(e.target.value)}
              placeholder={`Give ${agentName || 'your agent'} a mission...`}
              className="mc-goal-input"
              disabled={stream.starting}
              autoFocus
            />
            <button
              type="submit"
              className="mc-launch-btn"
              disabled={stream.starting || !goalInput.trim()}
            >
              {stream.starting ? 'Launching...' : 'Launch'}
            </button>
          </form>
        ) : (
          <div className="mc-goal-display">
            <div className="mc-goal-text">{currentGoal}</div>
            <div className="mc-goal-meta">
              <span className={`mc-status mc-status-${stream.status}`}>
                {stream.status.replace(/_/g, ' ')}
              </span>
              <span className="mc-meta-item">Tier: {tierLabel}</span>
              <span className="mc-meta-item">
                Turn {stream.turn}/{maxTurns}
              </span>
              <span className="mc-meta-item">
                Tools: {stream.toolCallCount}
              </span>
              {stream.isActive && (
                <button className="mc-cancel-btn" onClick={stream.cancel} aria-label="Cancel current mission">
                  Cancel
                </button>
              )}
              {!stream.isActive && (
                <button className="mc-new-btn" onClick={handleReset} aria-label="Start a new mission">
                  New Mission
                </button>
              )}
            </div>
          </div>
        )}

        {/* Active thought stream */}
        {stream.status === 'running' && stream.activeThought && (
          <div className="mc-thought">
            <div className="mc-thought-indicator" />
            <div className="mc-thought-text">{stream.activeThought}</div>
          </div>
        )}

        {/* Done summary */}
        {stream.doneSummary && !stream.isActive && (
          <div className={`mc-summary mc-summary-${stream.status}`}>
            <strong>{stream.status === 'completed' ? 'Completed' : stream.status === 'failed' ? 'Failed' : 'Cancelled'}:</strong>{' '}
            {stream.doneSummary}
          </div>
        )}

        {/* ===== Mentor modal (blocking overlay) ===== */}
        {stream.status === 'waiting_for_mentor' && stream.mentorQuestion && (
          <div className="mc-modal-overlay">
            <div className="mc-modal" ref={mentorTrapRef} role="dialog" aria-modal="true" aria-label="Mentor input needed">
              <div className="mc-modal-header">Input Needed</div>
              <div className="mc-modal-question">{stream.mentorQuestion}</div>
              <div className="mc-modal-form">
                <textarea
                  value={mentorInput}
                  onChange={(e) => setMentorInput(e.target.value)}
                  placeholder="Your response..."
                  className="mc-modal-input"
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && !e.shiftKey) {
                      e.preventDefault();
                      handleMentorRespond();
                    }
                  }}
                />
                <button
                  className="mc-launch-btn"
                  onClick={handleMentorRespond}
                  disabled={!mentorInput.trim()}
                >
                  Respond
                </button>
              </div>
            </div>
          </div>
        )}

        {/* ===== Confirmation modal ===== */}
        {stream.status === 'waiting_for_confirmation' && stream.confirmationInfo && (
          <div className="mc-modal-overlay">
            <div className="mc-modal" ref={confirmTrapRef} role="dialog" aria-modal="true" aria-label="Confirm tool execution">
              <div className="mc-modal-header">Confirm Tool Execution</div>
              <div className="mc-modal-question">
                <strong>{stream.confirmationInfo.toolName}</strong>
                <pre className="mc-modal-args">{stream.confirmationInfo.args}</pre>
              </div>
              <div className="mc-modal-actions">
                <button
                  className="mc-approve-btn"
                  onClick={() => stream.confirm(true)}
                >
                  Approve
                </button>
                <button
                  className="mc-deny-btn"
                  onClick={() => stream.confirm(false)}
                >
                  Deny
                </button>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* ==================== PERIPHERY ==================== */}
      <div className="mc-periphery">
        {/* Log toggle */}
        <button
          className={`mc-periph-toggle${hasErrors ? ' mc-periph-toggle-error' : ''}`}
          onClick={() => setShowLog(!showLog)}
          aria-expanded={showLog}
          aria-label={showLog ? 'Hide activity log' : 'Show activity log'}
        >
          {showLog ? 'Hide' : 'Show'} Log
          {hasErrors && ' (!)'}
          <span className="mc-periph-count">{stream.entries.length} entries</span>
        </button>

        {showLog && (
          <div className="mc-log">
            {stream.entries.length === 0 ? (
              <div className="mc-log-empty">No activity yet.</div>
            ) : (
              stream.entries.map((entry) => (
                <div
                  key={entry.id}
                  className={`mc-log-entry mc-log-${entry.type}${entry.success === false ? ' mc-log-error' : ''}`}
                >
                  <span className="mc-log-label">
                    {entry.toolName
                      ? `${entry.type}: ${entry.toolName}`
                      : entry.type.replace(/_/g, ' ')}
                  </span>
                  <span className="mc-log-turn">T{entry.turn}</span>
                  <div className="mc-log-content">{entry.content}</div>
                </div>
              ))
            )}
          </div>
        )}

        {/* Skill registry */}
        <button
          className="mc-periph-toggle"
          onClick={() => setShowSkills(!showSkills)}
          aria-expanded={showSkills}
          aria-label={showSkills ? 'Hide skill registry' : 'Show skill registry'}
        >
          {showSkills ? 'Hide' : 'Show'} Skills
          <span className="mc-periph-count">{skills.length} registered</span>
        </button>

        {showSkills && (
          <div className="mc-skills">
            {skills.length === 0 ? (
              <div className="mc-skills-empty">No skills registered.</div>
            ) : (
              skills.map((skill) => {
                const st = skillStatus(skill);
                const missing = missingSecrets.filter((m) => m.skill_id === skill.id);
                return (
                  <div key={skill.id} className="mc-skill">
                    <span className={`mc-skill-dot mc-skill-dot-${st}`} />
                    <span className="mc-skill-name">{skill.name}</span>
                    <span className="mc-skill-tier">{skill.trust_tier}</span>
                    <span className="mc-skill-tools">
                      {skill.tools.length} tool{skill.tools.length !== 1 ? 's' : ''}
                    </span>
                    {missing.length > 0 && (
                      <span className="mc-skill-missing">
                        Needs: {missing.map((m) => m.secret_name).join(', ')}
                      </span>
                    )}
                  </div>
                );
              })
            )}
          </div>
        )}
      </div>
    </div>
  );
}
