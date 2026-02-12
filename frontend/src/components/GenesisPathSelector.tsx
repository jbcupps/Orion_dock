import { useEffect, useState } from 'react';
import { fetchGenesisPaths, startGenesis, type GenesisPathItem } from '../api';
import './GenesisPathSelector.css';

interface GenesisPathSelectorProps {
  agentId: string;
  onStarted: (
    path: string,
    data?: { state?: string; prompt?: string; choices?: string[] }
  ) => void | Promise<void>;
  onError: (message: string) => void;
}

function pathLabel(item: GenesisPathItem): string {
  if (item.depth) {
    const depthLabel =
      item.depth === 'quick_start'
        ? 'Quick Start'
        : item.depth === 'conversation'
          ? 'Conversation'
          : item.depth === 'deep_dive'
            ? 'Deep Dive'
            : item.depth;
    return `${item.label} (${depthLabel})`;
  }
  return item.label;
}

export default function GenesisPathSelector({
  agentId,
  onStarted,
  onError,
}: GenesisPathSelectorProps) {
  const [paths, setPaths] = useState<GenesisPathItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [starting, setStarting] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = await fetchGenesisPaths();
        // Filter out paths that don't have a working frontend yet
        const working = list.filter((p) => p.id !== 'soul_crystallization');
        if (!cancelled) setPaths(working);
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

  const handleSelect = async (item: GenesisPathItem) => {
    const key = item.depth ? `${item.id}:${item.depth}` : item.id;
    if (starting) return;
    try {
      setStarting(key);
      const data = await startGenesis(agentId, item.id, item.depth);
      await Promise.resolve(
        onStarted(data.path, {
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
        <p>Loading Genesis paths…</p>
      </div>
    );
  }

  return (
    <div className="genesis-path-selector">
      <h3 className="genesis-path-title">Choose how to discover your agent</h3>
      <p className="genesis-path-subtitle">
        Each path leads to the same outcome — a soul document — but the journey differs.
      </p>
      <div className="genesis-path-cards">
        {paths.map((item) => {
          const key = item.depth ? `${item.id}:${item.depth}` : item.id;
          const isStarting = starting === key;
          return (
            <button
              key={key}
              type="button"
              className="genesis-path-card"
              onClick={() => handleSelect(item)}
              disabled={!!starting}
              title={item.estimated_time}
            >
              <span className="genesis-path-card-label">{pathLabel(item)}</span>
              <span className="genesis-path-card-time">{item.estimated_time}</span>
              <p className="genesis-path-card-desc">{item.description}</p>
              {isStarting && <span className="genesis-path-card-busy">Starting…</span>}
            </button>
          );
        })}
      </div>
    </div>
  );
}
