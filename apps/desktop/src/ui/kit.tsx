import { useId, useState, type ButtonHTMLAttributes, type ReactNode } from 'react';
import { AlertTriangle, CheckCircle2, Info, Loader2, XCircle } from 'lucide-react';

export const cn = (...parts: (string | false | null | undefined)[]): string =>
  parts.filter(Boolean).join(' ');

// --- Button -----------------------------------------------------------------------------------

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger';
type Size = 'sm' | 'md' | 'lg';

const VARIANT: Record<Variant, string> = {
  primary: 'bg-accent text-accent-fg hover:bg-accent-hover shadow-sm',
  secondary: 'bg-surface text-fg border border-line-strong hover:bg-surface-2',
  ghost: 'text-muted hover:bg-surface-2 hover:text-fg',
  danger: 'bg-danger text-accent-fg hover:opacity-90',
};
const SIZE: Record<Size, string> = {
  sm: 'h-8 px-3 text-[13px] gap-1.5',
  md: 'h-9 px-4 gap-2',
  lg: 'h-11 px-5 text-[15px] gap-2',
};

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  icon?: ReactNode;
  loading?: boolean;
}

export function Button({
  variant = 'secondary',
  size = 'md',
  icon,
  loading,
  className,
  children,
  disabled,
  type = 'button',
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled === true || loading === true}
      aria-busy={loading || undefined}
      className={cn(
        'inline-flex shrink-0 items-center justify-center rounded-control font-medium whitespace-nowrap transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-50',
        VARIANT[variant],
        SIZE[size],
        className,
      )}
      {...rest}
    >
      {loading ? <Loader2 className="size-4 cm-spin" aria-hidden /> : icon}
      {children}
    </button>
  );
}

export function IconButton({
  label,
  className,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={cn(
        'inline-flex size-9 items-center justify-center rounded-control text-muted transition-colors hover:bg-surface-2 hover:text-fg',
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}

// --- Badge / status ---------------------------------------------------------------------------

export type Tone = 'neutral' | 'accent' | 'success' | 'warning' | 'danger';

const TONE_SOFT: Record<Tone, string> = {
  neutral: 'bg-surface-2 text-muted',
  accent: 'bg-accent-soft text-accent-text',
  success: 'bg-success-soft text-success',
  warning: 'bg-warning-soft text-warning',
  danger: 'bg-danger-soft text-danger',
};

export function Badge({
  tone = 'neutral',
  dot,
  children,
  className,
}: {
  tone?: Tone;
  dot?: boolean;
  children: ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium',
        TONE_SOFT[tone],
        className,
      )}
    >
      {dot ? <span className="size-1.5 rounded-full bg-current" aria-hidden /> : null}
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
} as const;

export function Banner({
  tone = 'neutral',
  title,
  children,
  action,
  role,
}: {
  tone?: Tone;
  title?: ReactNode;
  children?: ReactNode;
  action?: ReactNode;
  role?: 'alert' | 'status';
}) {
  const Icon = BANNER_ICON[tone];
  return (
    <div
      role={role}
      className={cn(
        'flex items-start gap-3 rounded-xl px-4 py-3 text-sm',
        tone === 'neutral' ? 'bg-surface-2 text-fg' : TONE_SOFT[tone],
      )}
    >
      <Icon className="mt-0.5 size-4 shrink-0" aria-hidden />
      <div className="min-w-0 flex-1">
        {title ? <p className="font-medium">{title}</p> : null}
        {children ? (
          <div
            className={cn(
              'text-[13px]',
              title ? 'mt-0.5' : false,
              tone === 'neutral' ? 'text-muted' : 'opacity-90',
            )}
          >
            {children}
          </div>
        ) : null}
      </div>
      {action ? <div className="shrink-0">{action}</div> : null}
    </div>
  );
}

// --- Switch -----------------------------------------------------------------------------------

export function Switch({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        'relative inline-flex h-6 w-10 shrink-0 items-center rounded-full transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-50',
        checked ? 'bg-accent' : 'bg-line-strong',
      )}
    >
      <span
        aria-hidden
        className={cn(
          'inline-block size-[18px] rounded-full bg-white shadow transition-transform duration-150',
          checked ? 'translate-x-[18px]' : 'translate-x-[3px]',
        )}
      />
    </button>
  );
}

/** Label + optional description on the left, a control on the right. */
export function Row({
  label,
  description,
  children,
}: {
  label: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-6 py-3">
      <div className="min-w-0">
        <p className="text-sm font-medium text-fg">{label}</p>
        {description ? <p className="mt-0.5 text-[13px] text-muted">{description}</p> : null}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

export function SwitchRow({
  label,
  description,
  checked,
  onChange,
  disabled,
}: {
  label: string;
  description?: ReactNode;
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <Row label={label} description={description}>
      <Switch label={label} checked={checked} onChange={onChange} disabled={disabled} />
    </Row>
  );
}

// --- Segmented control ------------------------------------------------------------------------

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  title?: string;
  busy?: boolean;
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  size = 'md',
  full,
  disabled,
}: {
  value: T | null;
  options: readonly SegmentedOption<T>[];
  onChange: (v: T) => void;
  label: string;
  size?: 'sm' | 'md';
  full?: boolean;
  disabled?: boolean;
}) {
  const move = (dir: 1 | -1, from: number) => {
    const next = options[(from + dir + options.length) % options.length];
    if (next) onChange(next.value);
  };
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className={cn('inline-flex gap-0.5 rounded-[11px] bg-surface-2 p-0.5', full && 'flex w-full')}
    >
      {options.map((o, i) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            title={o.title}
            disabled={disabled}
            tabIndex={on || (value === null && i === 0) ? 0 : -1}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => {
              if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
                e.preventDefault();
                move(1, i);
              } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
                e.preventDefault();
                move(-1, i);
              }
            }}
            className={cn(
              'inline-flex items-center justify-center gap-1.5 rounded-[9px] font-medium transition-all duration-150 disabled:opacity-50',
              size === 'sm' ? 'h-7 px-2.5 text-[12.5px]' : 'h-8 px-3.5 text-[13px]',
              full && 'flex-1',
              on ? 'bg-surface text-fg shadow-sm' : 'text-muted hover:text-fg',
            )}
          >
            {o.busy ? <Loader2 className="size-3 cm-spin" aria-hidden /> : null}
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

// --- Slider -----------------------------------------------------------------------------------

export function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  format,
  onCommit,
  disabled,
  track,
  hideLabel,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  format?: (v: number) => string;
  onCommit: (v: number) => void;
  disabled?: boolean;
  /** CSS background for the track (e.g. a kelvin gradient). Defaults to an accent fill. */
  track?: string;
  hideLabel?: boolean;
}) {
  const id = useId();
  const [draft, setDraft] = useState<number | null>(null);
  const [prev, setPrev] = useState(value);
  // The daemon's confirmation replaces the local draft.
  if (prev !== value) {
    setPrev(value);
    setDraft(null);
  }
  const shown = draft ?? value;
  const fill = max === min ? 0 : ((shown - min) / (max - min)) * 100;
  const commit = () => {
    if (draft !== null && draft !== value) onCommit(draft);
    else setDraft(null);
  };
  return (
    <div>
      <div className={cn('mb-1 flex items-baseline justify-between', hideLabel && 'sr-only')}>
        <label htmlFor={id} className="text-[13px] font-medium text-fg">
          {label}
        </label>
        <output htmlFor={id} className="text-[13px] text-muted tabular">
          {format ? format(shown) : shown}
        </output>
      </div>
      <input
        id={id}
        type="range"
        className="cm-range"
        min={min}
        max={max}
        step={step}
        value={shown}
        disabled={disabled}
        aria-valuetext={format ? format(shown) : undefined}
        style={{
          ['--fill' as string]: `${fill}%`,
          ...(track ? { ['--track' as string]: track } : {}),
        }}
        onChange={(e) => setDraft(Number(e.target.value))}
        onPointerUp={commit}
        onKeyUp={commit}
        onBlur={commit}
      />
    </div>
  );
}

// --- Fields -----------------------------------------------------------------------------------

const FIELD =
  'h-9 w-full rounded-control border border-line-strong bg-surface px-3 text-sm text-fg placeholder:text-subtle disabled:opacity-50';

export function TextField({
  label,
  value,
  onChange,
  type = 'text',
  placeholder,
  error,
  hint,
  mono,
  disabled,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  type?: 'text' | 'password' | 'number';
  placeholder?: string;
  error?: string;
  hint?: string;
  mono?: boolean;
  disabled?: boolean;
}) {
  const id = useId();
  return (
    <div>
      <label htmlFor={id} className="mb-1 block text-[13px] font-medium text-fg">
        {label}
      </label>
      <input
        id={id}
        type={type}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        aria-invalid={error ? true : undefined}
        aria-describedby={error || hint ? `${id}-d` : undefined}
        onChange={(e) => onChange(e.target.value)}
        className={cn(FIELD, mono && 'font-mono', error && 'border-danger')}
      />
      {error || hint ? (
        <p id={`${id}-d`} className={cn('mt-1 text-xs', error ? 'text-danger' : 'text-subtle')}>
          {error ?? hint}
        </p>
      ) : null}
    </div>
  );
}

export function SelectField({
  label,
  value,
  options,
  onChange,
  disabled,
}: {
  label: string;
  value: string;
  options: readonly { value: string; label: string }[];
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  const id = useId();
  return (
    <div>
      <label htmlFor={id} className="mb-1 block text-[13px] font-medium text-fg">
        {label}
      </label>
      <select
        id={id}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        className={FIELD}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  );
}

// --- Layout bits ------------------------------------------------------------------------------

export function KeyValue({ label, children }: { label: ReactNode; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 py-2 text-sm">
      <dt className="text-muted">{label}</dt>
      <dd className="min-w-0 text-right text-fg tabular">{children}</dd>
    </div>
  );
}

export function SectionTitle({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="mb-2 mt-6 flex items-center justify-between first:mt-0">
      <h3 className="text-xs font-semibold uppercase tracking-wide text-subtle">{children}</h3>
      {aside}
    </div>
  );
}

export function Spinner({ label }: { label: string }) {
  return (
    <span role="status" className="inline-flex items-center gap-2 text-sm text-muted">
      <Loader2 className="size-4 cm-spin" aria-hidden />
      {label}
    </span>
  );
}

export function EmptyNote({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="rounded-xl border border-dashed border-line-strong px-4 py-6 text-center">
      <p className="text-sm font-medium text-fg">{title}</p>
      {children ? <p className="mx-auto mt-1 max-w-sm text-[13px] text-muted">{children}</p> : null}
    </div>
  );
}
