import { Component, type ErrorInfo, type ReactNode } from 'react';
import { useI18n } from '../i18n';
import { Button } from './ui/Button';
import { EmptyState } from './ui/Layout';

function Fallback({ error, reset }: { error: Error; reset: () => void }) {
  const { t } = useI18n();
  return (
    <div role="alert" className="p-6">
      <EmptyState
        title={t('error.boundary.title')}
        actions={<Button onClick={reset}>{t('error.boundary.retry')}</Button>}
      >
        <p>{t('error.boundary.body')}</p>
        <p className="mt-2 break-words font-mono text-xs text-subtle">{error.message}</p>
      </EmptyState>
    </div>
  );
}

interface Props {
  children: ReactNode;
  /** Changing this value clears a previous error (e.g. when the user navigates). */
  resetKey?: string;
}
interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('UI error', error, info.componentStack);
  }

  override componentDidUpdate(prev: Props): void {
    if (this.state.error && prev.resetKey !== this.props.resetKey) this.setState({ error: null });
  }

  override render(): ReactNode {
    if (this.state.error) {
      return <Fallback error={this.state.error} reset={() => this.setState({ error: null })} />;
    }
    return this.props.children;
  }
}
