import { useEffect, useState } from 'react';
import { fetchGenesisPaths, startGenesis, type GenesisPathItem } from '../api';
import './GenesisPathSelector.css';

interface GenesisPathSelectorProps {
  agentId: string;
  onStarted: (
    path: string,
    data?: { completed?: boolean; state?: string; prompt?: string; choices?: string[] }
  ) => void | Promise<void>;
  onError: (message: string) => void;
}

const DEPTH_OPTIONS: { value: string; label: string; time: string }[] = [
  { value: 'quick_start', label: 'Quick Start', time: '~30 sec' },
  { value: 'conversation', label: 'Conversation', time: '3-5 min' },
  { value: 'deep_dive', label: 'Deep Dive', time: '10-15 min' },
];

export default function GenesisPathSelector({
  agentId,
  onStarted,
  onError,
}: GenesisPathSelectorProps) {
  const [paths, setPaths] = useState<GenesisPathItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [starting, setStarting] = useState<string | null>(null);
  const [expandedCrystallization, setExpandedCrystallization] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = await fetchGenesisPaths();
        if (!cancelled) setPaths(list);
      } catch {
        if (!cancelled) setPaths([]);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const handleSelect = async (item: GenesisPathItem, depthOverride?: string) => {
    if (starting) return;

    // For Soul Crystallization, show depth sub-selector first
    if (item.id === 'soul_crystallization' && !depthOverride) {
      setExpandedCrystallization(true);
      return;
    }

    const depth = depthOverride ?? item.depth;
    const key = depth ? `${item.id}:${depth}` : item.id;
    try {
      setStarting(key);
      const data = await startGenesis(agentId, item.id, depth);
      await Promise.resolve(
        onStarted(data.path, {
          completed: data.completed,
          state: data.state,
          prompt: data.prompt,
          choices: data.choices,
        })
      );
    } catch (e) {
      onError(e instanceof Error ? e.message : 'Failed to start Genesis');
    } finally {
      setStarting(null);
    }
  };

  if (loading) {
    return (
      <div className="genesis-paths-loading">
        <p>Loading Genesis paths...</p>
      </div>
    );
  }

  return (
    <div className="genesis-path-selector">
      <h3 className="genesis-path-title">Choose how to discover your agent</h3>
      <p className="genesis-path-subtitle">
        Each path leads to the same outcome &mdash; a soul document &mdash; but the journey differs.
      </p>
      <div className="genesis-path-cards">
        {paths.map((item) => {
          const key = item.id;
          const isCrystallization = item.id === 'soul_crystallization';
          const isExpanded = isCrystallization && expandedCrystallization;

          return (
            <div
              key={key}
              className={`genesis-path-card${isExpanded ? ' genesis-path-card-expanded' : ''}`}
            >
              <button
                type="button"
                className="genesis-path-card-btn"
                onClick={() => handleSelect(item)}
                disabled={!!starting}
              >
                <span className="genesis-path-card-label">{item.label}</span>
                <span className="genesis-path-card-time">{item.estimated_time}</span>
                <p className="genesis-path-card-desc">{item.description}</p>
                {starting === key && (
                  <span className="genesis-path-card-busy">Starting...</span>
                )}
              </button>

              {isExpanded && (
                <div className="genesis-depth-selector">
                  <p className="genesis-depth-label">Choose depth:</p>
                  <div className="genesis-depth-options">
                    {DEPTH_OPTIONS.map((d) => {
                      const isStartingThis = starting === `${item.id}:${d.value}`;
                      return (
                        <button
                          key={d.value}
                          type="button"
                          className="genesis-depth-btn"
                          onClick={() => handleSelect(item, d.value)}
                          disabled={!!starting}
                        >
                          <span className="genesis-depth-btn-label">{d.label}</span>
                          <span className="genesis-depth-btn-time">{d.time}</span>
                          {isStartingThis && <span className="genesis-path-card-busy">Starting...</span>}
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
