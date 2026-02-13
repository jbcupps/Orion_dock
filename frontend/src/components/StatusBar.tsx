import './StatusBar.css';

export type HealthState = 'ok' | 'pending' | 'error';

export interface StatusBarProps {
  health: HealthState;
  birthModelReady: boolean | null;
  localLlmConfigured: boolean;
  birthModelName: string | null;
}

export default function StatusBar({
  health,
  birthModelReady,
  localLlmConfigured,
  birthModelName,
}: StatusBarProps) {
  const apiStatus = health === 'ok' ? 'ok' : health === 'error' ? 'error' : 'pending';

  let modelStatus: 'ready' | 'unavailable' | 'not_configured' | 'pending';
  let modelLabel: string;
  if (!localLlmConfigured) {
    modelStatus = 'not_configured';
    modelLabel = 'Birth model';
  } else if (birthModelReady === true) {
    modelStatus = 'ready';
    modelLabel = birthModelName ? `Birth model (${birthModelName})` : 'Birth model';
  } else if (birthModelReady === false) {
    modelStatus = 'unavailable';
    modelLabel = birthModelName ? `Birth model (${birthModelName})` : 'Birth model';
  } else {
    modelStatus = 'pending';
    modelLabel = 'Birth model';
  }

  return (
    <footer className="status-bar" role="status" aria-label="System status">
      <span className={`status-bar-item status-bar-${apiStatus}`} title="API health">
        {apiStatus === 'ok' && '●'}
        {apiStatus === 'pending' && '○'}
        {apiStatus === 'error' && '✕'}
        {' '}
        API
      </span>
      <span className="status-bar-sep" aria-hidden>|</span>
      <span
        className={`status-bar-item status-bar-${modelStatus}`}
        title={modelStatus === 'ready' ? 'Birth model available' : modelStatus === 'unavailable' ? 'Birth model not found on LLM server' : modelStatus === 'not_configured' ? 'Local LLM not configured' : 'Checking…'}
      >
        {modelStatus === 'ready' && '●'}
        {modelStatus === 'pending' && '○'}
        {(modelStatus === 'unavailable' || modelStatus === 'not_configured') && '—'}
        {' '}
        {modelLabel}
      </span>
    </footer>
  );
}
