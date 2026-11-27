import { useEffect, useRef, type ReactNode } from 'react';
import { X } from 'lucide-react';
import { useI18n } from '../i18n';
import { useDaemon } from '../store/daemon';
import { Button, IconButton, cn } from './kit';

/** Opens/closes a native <dialog> from a boolean (jsdom has no showModal: fall back to `open`). */
function useDialog(open: boolean) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      if (typeof d.showModal === 'function') d.showModal();
      else d.setAttribute('open', '');
    } else if (!open && d.open) {
      if (typeof d.close === 'function') d.close();
      else d.removeAttribute('open');
    }
  }, [open]);
  return ref;
}

/** Right-hand detail panel. Esc, backdrop click and the close button all call `onClose`. */
export function Sheet({
  open,
  onClose,
  title,
  subtitle,
  width = 480,
  children,
  footer,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  subtitle?: ReactNode;
  width?: number;
  children: ReactNode;
  footer?: ReactNode;
}) {
  const { t } = useI18n();
  const ref = useDialog(open);
  return (
    <dialog
      ref={ref}
      className="cm-sheet"
      aria-label={title}
      style={{ ['--sheet-w' as string]: `${width}px` }}
      onClose={onClose}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      {open ? (
        <div className="flex h-full flex-col">
          <header className="flex items-start justify-between gap-4 px-6 pt-5 pb-4">
            <div className="min-w-0">
              <h2 className="text-lg font-semibold tracking-tight text-fg">{title}</h2>
              {subtitle ? <p className="mt-0.5 text-[13px] text-muted">{subtitle}</p> : null}
            </div>
            <IconButton label={t('common.close')} onClick={onClose} className="-mr-2">
              <X className="size-[18px]" aria-hidden />
            </IconButton>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-6">{children}</div>
          {footer ? <footer className="border-t border-line px-6 py-3">{footer}</footer> : null}
        </div>
      ) : null}
    </dialog>
  );
}

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  tone = 'primary',
  loading,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  description: string;
  confirmLabel: string;
  tone?: 'primary' | 'danger';
  loading?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const ref = useDialog(open);
  return (
    <dialog
      ref={ref}
      className="cm-modal"
      aria-label={title}
      onClose={onCancel}
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      {open ? (
        <div className="p-6">
          <h2 className="text-base font-semibold text-fg">{title}</h2>
          <p className="mt-2 text-sm text-muted">{description}</p>
          <div className="mt-6 flex justify-end gap-2">
            <Button variant="ghost" onClick={onCancel}>
              {t('common.cancel')}
            </Button>
            <Button
              variant={tone === 'danger' ? 'danger' : 'primary'}
              loading={loading}
              onClick={onConfirm}
            >
              {confirmLabel}
            </Button>
          </div>
        </div>
      ) : null}
    </dialog>
  );
}

/** Command palette shell: the content (input + list) is supplied by the caller. */
export function PaletteDialog({
  open,
  onClose,
  label,
  children,
}: {
  open: boolean;
  onClose: () => void;
  label: string;
  children: ReactNode;
}) {
  const ref = useDialog(open);
  return (
    <dialog
      ref={ref}
      className="cm-palette"
      aria-label={label}
      onClose={onClose}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      {open ? children : null}
    </dialog>
  );
}

const TOAST_TONE = {
  success: 'border-success/40',
  error: 'border-danger/50',
  info: 'border-line-strong',
} as const;

export function Toasts() {
  const toasts = useDaemon((s) => s.toasts);
  const dismiss = useDaemon((s) => s.dismissToast);
  const { t } = useI18n();
  return (
    <div
      className="pointer-events-none fixed inset-x-0 bottom-5 z-50 flex flex-col items-center gap-2 px-4"
      aria-live="polite"
    >
      {toasts.map((toast) => (
        <ToastItem key={toast.id} id={toast.id} onDone={dismiss}>
          <div
            role={toast.kind === 'error' ? 'alert' : 'status'}
            className={cn(
              'cm-enter pointer-events-auto flex max-w-md items-start gap-3 rounded-xl border bg-surface px-4 py-3 text-sm text-fg shadow-lg',
              TOAST_TONE[toast.kind],
            )}
          >
            <div className="min-w-0">
              <p className="font-medium">{toast.message}</p>
              {toast.detail ? (
                <p className="mt-0.5 text-[13px] text-muted">{toast.detail}</p>
              ) : null}
            </div>
            <button
              type="button"
              aria-label={t('common.dismiss')}
              className="-mr-1 text-subtle hover:text-fg"
              onClick={() => dismiss(toast.id)}
            >
              <X className="size-4" aria-hidden />
            </button>
          </div>
        </ToastItem>
      ))}
    </div>
  );
}

function ToastItem({
  id,
  onDone,
  children,
}: {
  id: number;
  onDone: (id: number) => void;
  children: ReactNode;
}) {
  useEffect(() => {
    const timer = setTimeout(() => onDone(id), 4500);
    return () => clearTimeout(timer);
  }, [id, onDone]);
  return <>{children}</>;
}
