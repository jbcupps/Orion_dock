import { useEffect, useState } from 'react';
import {
  fetchIdentities,
  createAgent,
  loadAgent,
  exportAgent,
  type AgentIdentityInfo,
} from '../api';

interface HiveScreenProps {
  onAgentSelected: (agentId: string) => void;
  onCreateAgent: (agentId: string) => void;
  onViewIntro?: () => void;
}

export default function HiveScreen({
  onAgentSelected,
  onCreateAgent,
  onViewIntro,
}: HiveScreenProps) {
  const [agents, setAgents] = useState<AgentIdentityInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [newAgentName, setNewAgentName] = useState('');
  const [exportingId, setExportingId] = useState<string | null>(null);
  const [exportModalAgent, setExportModalAgent] = useState<AgentIdentityInfo | null>(null);
  const [exportPrivateKey, setExportPrivateKey] = useState('');

  const fetchAgents = async () => {
    try {
      setLoading(true);
      const list = await fetchIdentities();
      setAgents(list);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load identities');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchAgents();
  }, []);

  const handleCreateAgent = async () => {
    const name = newAgentName.trim();
    if (!name) return;

    try {
      setCreating(true);
      const { id } = await createAgent(name);
      setNewAgentName('');
      setCreating(false);
      await loadAgent(id);
      onCreateAgent(id);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to create agent');
      setCreating(false);
    }
  };

  const handleSelectAgent = async (agentId: string) => {
    try {
      await loadAgent(agentId);
      onAgentSelected(agentId);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load agent');
    }
  };

  const closeExportModal = () => {
    setExportModalAgent(null);
    setExportPrivateKey('');
  };

  const handleExport = (e: React.MouseEvent, agent: AgentIdentityInfo) => {
    e.stopPropagation(); // prevent row click navigation
    setExportModalAgent(agent);
    setExportPrivateKey('');
  };

  const handleConfirmExport = async () => {
    if (!exportModalAgent) return;
    const privateKey = exportPrivateKey.trim();
    if (!privateKey) {
      setError('Private key is required to unlock keychain export');
      return;
    }

    try {
      setExportingId(exportModalAgent.id);
      await exportAgent(exportModalAgent.id, privateKey, exportModalAgent.name);
      closeExportModal();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Export failed');
    } finally {
      setExportingId(null);
    }
  };

  if (loading) {
    return (
      <div className="hive-loading">
        <div className="hive-loading-pulse">Loading identities...</div>
      </div>
    );
  }

  const errorBanner = error && (
    <div className="hive-error">
      {error}
      <button type="button" className="hive-error-dismiss" onClick={() => setError(null)}>
        dismiss
      </button>
    </div>
  );

  if (agents.length === 0) {
    return (
      <div className="hive-root hive-empty">
        <div className="hive-header">
          <h1 className="hive-title">ORION HIVE</h1>
          <p className="hive-subtitle">Identity Management</p>
          {onViewIntro && (
            <button
              type="button"
              className="hive-intro-link"
              onClick={onViewIntro}
            >
              [intro]
            </button>
          )}
        </div>
        <p className="hive-empty-msg">Create your first agent to begin</p>
        {errorBanner}
        <div className="hive-create-card">
          <input
            type="text"
            value={newAgentName}
            onChange={(e) => setNewAgentName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleCreateAgent()}
            placeholder="Agent name..."
            className="hive-input"
            disabled={creating}
            autoFocus
          />
          <button
            type="button"
            className="hive-btn"
            onClick={handleCreateAgent}
            disabled={creating || !newAgentName.trim()}
          >
            {creating ? 'Creating...' : 'Create'}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="hive-root">
      <div className="hive-header">
        <h1 className="hive-title">ORION HIVE</h1>
        <p className="hive-subtitle">Identity Management</p>
        {onViewIntro && (
          <button
            type="button"
            className="hive-intro-link"
            onClick={onViewIntro}
          >
            [intro]
          </button>
        )}
      </div>

      {errorBanner}

      <section className="hive-section">
        <p className="hive-section-label">Select an Identity</p>
        <table className="hive-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>UUID</th>
              <th>Status</th>
              <th>Birth Date</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {agents.map((agent) => (
              <tr
                key={agent.id}
                onClick={() => handleSelectAgent(agent.id)}
                className="hive-row"
              >
                <td className="hive-cell-name">{agent.name}</td>
                <td className="hive-cell-uuid">
                  {agent.id.substring(0, 8)}...
                </td>
                <td className="hive-cell-status">
                  {agent.birth_complete ? (
                    <span className="hive-status-born">born</span>
                  ) : (
                    <span className="hive-status-unborn">unborn</span>
                  )}
                </td>
                <td className="hive-cell-date">
                  {agent.birth_date ?? '\u2014'}
                </td>
                <td className="hive-cell-actions">
                  {agent.birth_complete && (
                    <button
                      type="button"
                      className="hive-export-btn"
                      onClick={(e) => handleExport(e, agent)}
                      disabled={exportingId === agent.id}
                    >
                      {exportingId === agent.id ? 'Exporting...' : 'Export'}
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section className="hive-section hive-init">
        <p className="hive-section-label">Initialize New Agent</p>
        <div className="hive-create-row">
          <input
            type="text"
            value={newAgentName}
            onChange={(e) => setNewAgentName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleCreateAgent()}
            placeholder="Agent name..."
            className="hive-input"
            disabled={creating}
          />
          <button
            type="button"
            className="hive-btn"
            onClick={handleCreateAgent}
            disabled={creating || !newAgentName.trim()}
          >
            {creating ? 'Creating...' : 'Create'}
          </button>
        </div>
      </section>

      {exportModalAgent && (
        <div
          className="hive-modal-backdrop"
          onClick={(e) => {
            if (e.target === e.currentTarget && exportingId !== exportModalAgent.id) {
              closeExportModal();
            }
          }}
        >
          <div className="hive-modal-card" role="dialog" aria-modal="true" aria-labelledby="hive-export-title">
            <h3 id="hive-export-title">Unlock keychain for export</h3>
            <p className="hive-modal-text">
              Enter the external private key for <strong>{exportModalAgent.name}</strong>.
            </p>
            <input
              type="password"
              value={exportPrivateKey}
              onChange={(e) => setExportPrivateKey(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleConfirmExport()}
              className="hive-input"
              placeholder="Private key (base64)"
              autoFocus
              disabled={exportingId === exportModalAgent.id}
            />
            <div className="hive-modal-actions">
              <button
                type="button"
                className="hive-btn-secondary"
                onClick={closeExportModal}
                disabled={exportingId === exportModalAgent.id}
              >
                Cancel
              </button>
              <button
                type="button"
                className="hive-btn"
                onClick={handleConfirmExport}
                disabled={exportingId === exportModalAgent.id || !exportPrivateKey.trim()}
              >
                {exportingId === exportModalAgent.id ? 'Exporting...' : 'Export'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
