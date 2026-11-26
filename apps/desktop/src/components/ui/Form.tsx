import {
  useEffect,
  useId,
  useRef,
  useState,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { cn } from './Button';

// --- Switch ---------------------------------------------------------------------------------

interface SwitchProps {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: ReactNode;
  description?: ReactNode;
  disabled?: boolean;
  /** Warn-styled row (e.g. custom scripts). */
  tone?: 'default' | 'warning';
}

/** Labelled switch row: the visible label names the control (aria-labelledby). */
export function SwitchRow({ checked, onChange, label, description, disabled, tone }: SwitchProps) {
  const id = useId();
  return (
    <div className="flex items-start justify-between gap-4 py-3">
      <div className="min-w-0">
        <div id={`${id}-l`} className="text-sm font-medium text-fg">
          {label}
        </div>
        {description ? (
          <div
            id={`${id}-d`}
            className={cn('mt-0.5 text-xs', tone === 'warning' ? 'text-warning' : 'text-muted')}
          >
            {description}
          </div>
        ) : null}
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-labelledby={`${id}-l`}
        aria-describedby={description ? `${id}-d` : undefined}
        disabled={disabled}
        onClick={() => onChange(!checked)}
        className={cn(
          'relative mt-0.5 h-6 w-10 shrink-0 rounded-full border transition-colors',
          'disabled:cursor-not-allowed disabled:opacity-50',
          checked ? 'border-transparent bg-accent' : 'border-line-strong bg-surface-2',
        )}
      >
        <span
          aria-hidden
          className={cn(
            'absolute top-0.5 size-[18px] rounded-full shadow-sm transition-all',
            checked ? 'left-[18px] bg-accent-fg' : 'left-0.5 bg-subtle',
          )}
        />
      </button>
    </div>
  );
}

// --- Slider ---------------------------------------------------------------------------------

interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  /** Called while dragging (throttled) and once on release. */
  onCommit: (value: number) => void;
  format?: (v: number) => string;
  disabled?: boolean;
  /** CSS background for the track (e.g. a Kelvin gradient). Default: accent fill. */
  track?: string;
  hideLabel?: boolean;
  throttleMs?: number;
}

export function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  onCommit,
  format = String,
  disabled,
  track,
  hideLabel,
  throttleMs = 120,
}: SliderProps) {
  const id = useId();
  const [draft, setDraft] = useState<number | null>(null);
  const last = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const hold = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const shown = draft ?? value;

  // The daemon echoed our value (or changed it): follow the real state again.
  useEffect(() => {
    if (draft !== null && Math.abs(value - draft) < step / 2) setDraft(null);
  }, [value, draft, step]);
  useEffect(
    () => () => {
      clearTimeout(timer.current);
      clearTimeout(hold.current);
    },
    [],
  );

  const commit = (v: number) => {
    last.current = Date.now();
    onCommit(v);
    clearTimeout(hold.current);
    hold.current = setTimeout(() => setDraft(null), 2000);
  };
  const onInput = (v: number) => {
    setDraft(v);
    const wait = throttleMs - (Date.now() - last.current);
    clearTimeout(timer.current);
    if (wait <= 0) commit(v);
    else timer.current = setTimeout(() => commit(v), wait);
  };
  const fill = ((shown - min) / (max - min)) * 100;
  return (
    <div className="w-full">
      <div className={cn('mb-1 flex items-baseline justify-between', hideLabel && 'sr-only')}>
        <label htmlFor={id} className="text-sm font-medium text-fg">
          {label}
        </label>
        <span className="text-sm text-muted tabular-nums">{format(shown)}</span>
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
        aria-valuetext={format(shown)}
        style={
          { '--fill': `${fill}%`, ...(track ? { '--track': track } : {}) } as React.CSSProperties
        }
        onChange={(e) => onInput(Number(e.target.value))}
        onPointerUp={(e) => {
          clearTimeout(timer.current);
          commit(Number(e.currentTarget.value));
        }}
      />
    </div>
  );
}

// --- Segmented control ------------------------------------------------------------------------

interface SegmentedProps<T extends string> {
  label: string;
  value: T;
  options: readonly { value: T; label: ReactNode }[];
  onChange: (v: T) => void;
  disabled?: boolean;
}

/** Radio group rendered as a segmented control, with roving tabindex + arrow keys. */
export function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
}: SegmentedProps<T>) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const onKey = (e: KeyboardEvent, i: number) => {
    const dir =
      e.key === 'ArrowRight' || e.key === 'ArrowDown'
        ? 1
        : e.key === 'ArrowLeft' || e.key === 'ArrowUp'
          ? -1
          : 0;
    if (!dir) return;
    e.preventDefault();
    const n = (i + dir + options.length) % options.length;
    const opt = options[n];
    if (opt) {
      onChange(opt.value);
      refs.current[n]?.focus();
    }
  };
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className="inline-flex rounded-control border border-line-strong bg-surface-2 p-0.5"
    >
      {options.map((o, i) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on ? 0 : -1}
            disabled={disabled}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => onKey(e, i)}
            className={cn(
              'h-7 rounded-[6px] px-3 text-xs font-medium transition-colors disabled:opacity-50',
              on ? 'bg-accent-soft text-accent-text shadow-sm' : 'text-muted hover:text-fg',
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

// --- Fields -----------------------------------------------------------------------------------

interface FieldProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'onChange' | 'value'> {
  label: string;
  value: string;
  onChange: (v: string) => void;
  hint?: ReactNode;
  error?: ReactNode;
  mono?: boolean;
}

export function TextField({
  label,
  value,
  onChange,
  hint,
  error,
  mono,
  className,
  ...rest
}: FieldProps) {
  const id = useId();
  return (
    <div className={className}>
      <label htmlFor={id} className="mb-1 block text-xs font-medium text-muted">
        {label}
      </label>
      <input
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-invalid={error ? true : undefined}
        aria-describedby={hint || error ? `${id}-h` : undefined}
        className={cn(
          'h-9 w-full rounded-control border bg-surface px-3 text-sm text-fg placeholder:text-subtle',
          'disabled:opacity-50',
          error ? 'border-danger' : 'border-line-strong',
          mono && 'font-mono text-[13px]',
        )}
        {...rest}
      />
      {hint || error ? (
        <p id={`${id}-h`} className={cn('mt-1 text-xs', error ? 'text-danger' : 'text-subtle')}>
          {error ?? hint}
        </p>
      ) : null}
    </div>
  );
}

interface SelectFieldProps {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: readonly { value: string; label: string }[];
  disabled?: boolean;
  hideLabel?: boolean;
  className?: string;
}

export function SelectField({
  label,
  value,
  onChange,
  options,
  disabled,
  hideLabel,
  className,
}: SelectFieldProps) {
  const id = useId();
  return (
    <div className={className}>
      <label
        htmlFor={id}
        className={cn('mb-1 block text-xs font-medium text-muted', hideLabel && 'sr-only')}
      >
        {label}
      </label>
      <select
        id={id}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        className="h-9 w-full rounded-control border border-line-strong bg-surface px-2.5 text-sm text-fg disabled:opacity-50"
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
