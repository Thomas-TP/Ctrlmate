import type { ReactNode } from 'react';
import { AlertTriangle, CheckCircle2, Info, XCircle } from 'lucide-react';
import { cn } from './Button';

export function PageHeader({
  title,
  description,
  actions,
}: {
  title: string;
  description?: string;
  actions?: ReactNode;
}) {
  return (
    <header className="mb-6 flex flex-wrap items-end justify-between gap-3">
      <div className="min-w-0">
        <h1 className="text-xl font-semibold tracking-tight text-fg">{title}</h1>
        {description ? <p className="mt-1 max-w-2xl text-sm text-muted">{description}</p> : null}
      </div>
      {actions ? <div className="flex items-center gap-2">{actions}</div> : null}
    </header>
  );
}

export function Card({
  title,
  description,
  actions,
  children,
  className,
  id,
}: {
  title?: string;
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  id?: string;
}) {
  const headingId = id ? `${id}-h` : undefined;
  return (
    <section
      id={id}
      aria-labelledby={title ? headingId : undefined}
      className={cn('rounded-card border border-line bg-surface p-5', className)}
    >
      {title || actions ? (
        <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            {title ? (
              <h2 id={headingId} className="text-sm font-semibold text-fg">
                {title}
              </h2>
            ) : null}
            {description ? <p className="mt-0.5 text-xs text-muted">{description}</p> : null}
          </div>
          {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
        </div>
      ) : null}
      {children}
    </section>
  );
}

export type Tone = 'neutral' | 'accent' | 'success' | 'warning' | 'danger';

const TONE_BADGE: Record<Tone, string> = {
  neutral: 'bg-surface-2 text-muted',
  accent: 'bg-accent-soft text-accent-text',
  success: 'bg-success-soft text-success',
  warning: 'bg-warning-soft text-warning',
  danger: 'bg-danger-soft text-danger',
};
const TONE_DOT: Record<Tone, string> = {
  neutral: 'bg-subtle',
  accent: 'bg-accent',
  success: 'bg-success',
  warning: 'bg-warning',
  danger: 'bg-danger',
};

/** Status chip: always dot + text, never colour alone. */
export function Badge({ tone = 'neutral', children }: { tone?: Tone; children: ReactNode }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium',
        TONE_BADGE[tone],
      )}
    >
      <span aria-hidden className={cn('size-1.5 rounded-full', TONE_DOT[tone])} />
      {children}
    </span>
  );
}

const BANNER_ICON = {
  neutral: Info,
  accent: Info,
  success: CheckCircle2,
  warning: AlertTriangle,
  danger: XCircle,
};

export function Banner({
  tone = 'neutral',
  title,
  children,
  actions,
  role,
}: {
  tone?: Tone;
  title?: string;
  children?: ReactNode;
  actions?: ReactNode;
  role?: 'alert' | 'status';
}) {
  const Icon = BANNER_ICON[tone];
  const color: Record<Tone, string> = {
    neutral: 'border-line bg-surface-2 text-fg',
    accent: 'border-line bg-accent-soft text-fg',
    success: 'border-line bg-success-soft text-fg',
    warning: 'border-line bg-warning-soft text-fg',
    danger: 'border-line bg-danger-soft text-fg',
  };
  const iconColor: Record<Tone, string> = {
    neutral: 'text-muted',
    accent: 'text-accent-text',
    success: 'text-success',
    warning: 'text-warning',
    danger: 'text-danger',
  };
  return (
    <div
      role={role}
      className={cn('flex items-start gap-3 rounded-control border p-3.5', color[tone])}
    >
      <Icon className={cn('mt-0.5 size-4 shrink-0', iconColor[tone])} aria-hidden />
      <div className="min-w-0 flex-1 text-sm">
        {title ? <p className="font-medium">{title}</p> : null}
        {children ? <div className={cn('text-muted', title && 'mt-0.5')}>{children}</div> : null}
      </div>
      {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </div>
  );
}

export function EmptyState({
  icon,
  title,
  children,
  actions,
}: {
  icon?: ReactNode;
  title: string;
  children?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center px-6 py-10 text-center">
      {icon ? <div className="mb-3 text-subtle">{icon}</div> : null}
      <p className="text-sm font-medium text-fg">{title}</p>
      {children ? <div className="mt-1 max-w-md text-sm text-muted">{children}</div> : null}
      {actions ? <div className="mt-4 flex items-center gap-2">{actions}</div> : null}
    </div>
  );
}

export function KeyValue({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 py-1.5 text-sm">
      <dt className="text-muted">{label}</dt>
      <dd className="min-w-0 text-right font-medium text-fg tabular-nums">{children}</dd>
    </div>
  );
}

export function Divider() {
  return <hr className="my-1 border-0 border-t border-line" />;
}

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded border border-line-strong bg-surface-2 px-1.5 py-0.5 font-mono text-xs text-muted">
      {children}
    </kbd>
  );
}
