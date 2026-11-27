import { Component, type ErrorInfo, type ReactNode } from 'react';
import { useI18n } from '../i18n';
import { Button, EmptyNote } from './kit';

function Fallback({ error, reset }: { error: Error; reset: () => void }) {
  const { t } = useI18n();
  return (
    <div role="alert" className="mx-auto max-w-md p-8">
      <EmptyNote title={t('error.boundary.title')}>{t('error.boundary.body')}</EmptyNote>
      <p className="mt-3 text-center font-mono text-xs break-words text-subtle">{error.message}</p>
      <div className="mt-4 flex justify-center">
        <Button onClick={reset}>{t('error.boundary.retry')}</Button>
      </div>
    </div>
  );
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('UI error', error, info.componentStack);
  }

  override render(): ReactNode {
    if (this.state.error) {
      return <Fallback error={this.state.error} reset={() => this.setState({ error: null })} />;
    }
    return this.props.children;
  }
}
