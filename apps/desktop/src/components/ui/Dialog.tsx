import { useEffect, useId, useRef, type ReactNode } from 'react';
import { Button } from './Button';

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Keeps Tab inside `root`, closes on Escape and restores the previously focused element. */
export function useModalBehavior(
  open: boolean,
  root: React.RefObject<HTMLElement | null>,
  onClose: () => void,
  initialFocus?: React.RefObject<HTMLElement | null>,
) {
  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    (initialFocus?.current ?? root.current?.querySelector<HTMLElement>(FOCUSABLE))?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== 'Tab' || !root.current) return;
      const items = Array.from(root.current.querySelectorAll<HTMLElement>(FOCUSABLE));
      const first = items[0];
      const last = items[items.length - 1];
      if (!first || !last) return;
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('keydown', onKey, true);
      previous?.focus();
    };
  }, [open, root, onClose, initialFocus]);
}

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description: ReactNode;
  confirmLabel: string;
  cancelLabel: string;
  tone?: 'default' | 'danger';
  loading?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

/** Modal confirmation. Focus starts on "cancel" so Enter never fires a destructive action. */
export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  cancelLabel,
  tone = 'default',
  loading,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  useModalBehavior(open, root, onCancel, cancel);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div
        ref={root}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={`${id}-t`}
        aria-describedby={`${id}-d`}
        className="cm-enter w-full max-w-md rounded-card border border-line bg-surface p-6 shadow-xl"
      >
        <h2 id={`${id}-t`} className="text-base font-semibold text-fg">
          {title}
        </h2>
        <div id={`${id}-d`} className="mt-2 text-sm text-muted">
          {description}
        </div>
        <div className="mt-6 flex justify-end gap-2">
          <Button ref={cancel} onClick={onCancel}>
            {cancelLabel}
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
    </div>
  );
}
