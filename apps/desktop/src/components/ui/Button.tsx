import type { ComponentPropsWithRef, ReactNode } from 'react';
import { Loader2 } from 'lucide-react';

export const cn = (...parts: (string | false | null | undefined)[]): string =>
  parts.filter(Boolean).join(' ');

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';

interface ButtonProps extends ComponentPropsWithRef<'button'> {
  variant?: ButtonVariant;
  size?: 'sm' | 'md';
  loading?: boolean;
  icon?: ReactNode;
}

const VARIANTS: Record<ButtonVariant, string> = {
  primary: 'bg-accent text-accent-fg hover:bg-accent-hover border-transparent',
  secondary: 'bg-surface text-fg border-line-strong hover:bg-surface-2',
  ghost: 'bg-transparent text-muted border-transparent hover:bg-surface-2 hover:text-fg',
  danger: 'bg-danger text-accent-fg border-transparent hover:opacity-90',
};

export function Button({
  variant = 'secondary',
  size = 'md',
  loading = false,
  icon,
  className,
  children,
  disabled,
  type = 'button',
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      className={cn(
        'inline-flex items-center justify-center gap-2 rounded-control border font-medium transition-colors',
        'disabled:cursor-not-allowed disabled:opacity-50',
        size === 'sm' ? 'h-8 px-3 text-xs' : 'h-9 px-3.5 text-sm',
        VARIANTS[variant],
        className,
      )}
      {...rest}
    >
      {loading ? <Loader2 className="cm-spin size-4" aria-hidden /> : icon}
      {children}
    </button>
  );
}
