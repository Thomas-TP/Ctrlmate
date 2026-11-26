import { useEffect } from 'react';
import { X } from 'lucide-react';
import { useI18n } from '../../i18n';
import { useDaemon, type Toast } from '../../store/daemon';
import { cn } from './Button';

function ToastItem({ toast }: { toast: Toast }) {
  const { t } = useI18n();
  const dismiss = useDaemon((s) => s.dismissToast);
  useEffect(() => {
    const id = setTimeout(() => dismiss(toast.id), toast.kind === 'error' ? 9000 : 4500);
    return () => clearTimeout(id);
  }, [toast.id, toast.kind, dismiss]);
  return (
    <div
      role={toast.kind === 'error' ? 'alert' : 'status'}
      className={cn(
        'cm-enter pointer-events-auto flex items-start gap-3 rounded-control border bg-surface p-3 pr-2 text-sm shadow-lg',
        toast.kind === 'error' ? 'border-danger' : 'border-line-strong',
      )}
    >
      <span
        aria-hidden
        className={cn(
          'mt-1.5 size-2 shrink-0 rounded-full',
          toast.kind === 'error'
            ? 'bg-danger'
            : toast.kind === 'success'
              ? 'bg-success'
              : 'bg-accent',
        )}
      />
      <div className="min-w-0 flex-1">
        <p className="text-fg">{toast.message}</p>
        {toast.detail ? (
          <p className="mt-0.5 break-words font-mono text-xs text-subtle">{toast.detail}</p>
        ) : null}
      </div>
      <button
        type="button"
        onClick={() => dismiss(toast.id)}
        aria-label={t('common.dismiss')}
        className="rounded p-1 text-subtle hover:bg-surface-2 hover:text-fg"
      >
        <X className="size-4" aria-hidden />
      </button>
    </div>
  );
}

/** The live region stays mounted so assistive tech announces each toast as it appears. */
export function Toasts() {
  const toasts = useDaemon((s) => s.toasts);
  return (
    <div
      aria-live="polite"
      aria-atomic="false"
      className="pointer-events-none fixed right-4 bottom-4 z-[60] flex w-[min(380px,calc(100vw-2rem))] flex-col gap-2"
    >
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
    </div>
  );
}
