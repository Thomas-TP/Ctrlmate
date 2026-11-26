import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { Search } from 'lucide-react';
import { useI18n } from '../i18n';
import { useNav } from '../store/nav';
import { useCommands, type CommandGroup, type PaletteCommand } from './commands';
import { useModalBehavior } from './ui/Dialog';
import { Kbd } from './ui/Layout';
import { cn } from './ui/Button';

const GROUP_ORDER: CommandGroup[] = ['navigate', 'cooling', 'appearance', 'actions', 'app'];

/** Case- and accent-insensitive match on label + keywords. */
export function matches(cmd: Pick<PaletteCommand, 'label' | 'keywords'>, query: string): boolean {
  const norm = (s: string) => s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
  const hay = norm(`${cmd.label} ${cmd.keywords ?? ''}`);
  return norm(query)
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => hay.includes(w));
}

function Palette({ onClose }: { onClose: () => void }) {
  const { t } = useI18n();
  const commands = useCommands();
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const listId = useId();
  useModalBehavior(true, root, onClose, input);

  const results = useMemo(
    () =>
      commands
        .filter((c) => matches(c, query))
        .sort((a, b) => GROUP_ORDER.indexOf(a.group) - GROUP_ORDER.indexOf(b.group)),
    [commands, query],
  );
  const current = Math.min(active, Math.max(0, results.length - 1));

  useEffect(() => {
    document.getElementById(`${listId}-${current}`)?.scrollIntoView({ block: 'nearest' });
  }, [current, listId]);

  const run = (c: PaletteCommand) => {
    onClose();
    void c.run();
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActive((current + 1) % Math.max(1, results.length));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActive((current - 1 + results.length) % Math.max(1, results.length));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const c = results[current];
      if (c) run(c);
    }
  };

  let lastGroup: CommandGroup | null = null;
  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 p-4 pt-[12vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={root}
        role="dialog"
        aria-modal="true"
        aria-label={t('palette.title')}
        className="cm-enter w-full max-w-lg overflow-hidden rounded-card border border-line bg-surface shadow-xl"
      >
        <div className="flex items-center gap-3 border-b border-line px-4">
          <Search className="size-4 text-subtle" aria-hidden />
          <input
            ref={input}
            role="combobox"
            aria-expanded="true"
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={results.length ? `${listId}-${current}` : undefined}
            aria-label={t('palette.title')}
            placeholder={t('palette.placeholder')}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            onKeyDown={onKey}
            className="h-12 w-full bg-transparent text-sm text-fg outline-none placeholder:text-subtle"
          />
          <Kbd>Esc</Kbd>
        </div>
        <ul
          id={listId}
          role="listbox"
          aria-label={t('palette.title')}
          className="max-h-[50vh] overflow-auto p-2"
        >
          {results.length === 0 ? (
            <li role="presentation" className="px-3 py-6 text-center text-sm text-muted">
              {t('palette.empty')}
            </li>
          ) : null}
          {results.map((c, i) => {
            const header = c.group !== lastGroup;
            lastGroup = c.group;
            return (
              <li key={c.id} role="presentation">
                {header ? (
                  <p className="px-3 pt-2 pb-1 text-xs font-medium text-subtle">
                    {t(`palette.group.${c.group}`)}
                  </p>
                ) : null}
                <div
                  id={`${listId}-${i}`}
                  role="option"
                  aria-selected={i === current}
                  onMouseMove={() => setActive(i)}
                  onClick={() => run(c)}
                  className={cn(
                    'cursor-pointer rounded-control px-3 py-2 text-sm text-fg',
                    i === current && 'bg-accent-soft',
                  )}
                >
                  {c.label}
                </div>
              </li>
            );
          })}
        </ul>
      </div>
    </div>
  );
}

/** Mounts the palette on demand and owns the global Ctrl/Cmd+K shortcut. */
export function CommandPalette() {
  const open = useNav((s) => s.paletteOpen);
  const setOpen = useNav((s) => s.setPaletteOpen);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setOpen(!useNav.getState().paletteOpen);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [setOpen]);
  return open ? <Palette onClose={() => setOpen(false)} /> : null;
}
