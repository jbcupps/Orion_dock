import { Component, type ErrorInfo, type ReactNode } from 'react';

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

export default class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('ErrorBoundary caught:', error, info.componentStack);
  }

  render() {
    if (this.state.hasError) {
      return (
        <div style={{
          padding: '2rem',
          maxWidth: '600px',
          margin: '4rem auto',
          background: '#161b22',
          border: '1px solid #f85149',
          borderRadius: '8px',
          color: '#e6edf3',
          fontFamily: 'monospace',
        }}>
          <h2 style={{ color: '#f85149', marginTop: 0 }}>Something went wrong</h2>
          <p style={{ color: '#8b949e' }}>
            The application encountered an unexpected error. Try refreshing the page.
          </p>
          <pre style={{
            background: '#0d1117',
            padding: '1rem',
            borderRadius: '4px',
            overflow: 'auto',
            fontSize: '0.8rem',
            maxHeight: '200px',
          }}>
            {this.state.error?.message}
          </pre>
          <button
            onClick={() => window.location.reload()}
            style={{
              marginTop: '1rem',
              padding: '0.5rem 1rem',
              background: '#238636',
              color: '#fff',
              border: 'none',
              borderRadius: '4px',
              cursor: 'pointer',
              fontFamily: 'inherit',
            }}
          >
            Reload Page
          </button>
        </div>
      );
    }

    return this.props.children;
  }
}
