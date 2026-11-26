import { useEffect, type ReactNode } from 'react';
import {
  Fan,
  Home,
  LayoutDashboard,
  Lightbulb,
  Monitor,
  Search,
  Settings as SettingsIcon,
  type LucideIcon,
} from 'lucide-react';
import { useAppInfo } from '../hooks/hooks';
import { useI18n } from '../i18n';
import { useDaemon } from '../store/daemon';
import { SCREENS, useNav, type ScreenId } from '../store/nav';
import { cn } from './ui/Button';
import { Kbd } from './ui/Layout';

const ICONS: Record<ScreenId, LucideIcon> = {
  overview: LayoutDashboard,
  cooling: Fan,
  lighting: Lightbulb,
  system: Monitor,
  homeAssistant: Home,
  settings: SettingsIcon,
};

const isMac = () => typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent);

function Sidebar() {
  const { t } = useI18n();
  const screen = useNav((s) => s.screen);
  const setScreen = useNav((s) => s.setScreen);
  const status = useDaemon((s) => s.connection.status);
  const info = useAppInfo();
  return (
    <nav
      aria-label={t('nav.label')}
      className="flex w-14 shrink-0 flex-col border-r border-line bg-surface px-2 py-4 min-[900px]:w-56 min-[900px]:px-3"
    >
      <div className="mb-5 flex items-center gap-2 px-2">
        <span
          aria-hidden
          className="grid size-7 place-items-center rounded-[8px] bg-accent text-sm font-bold text-accent-fg"
        >
          C
        </span>
        <span className="text-sm font-semibold tracking-tight text-fg max-[899px]:sr-only">
          {t('app.name')}
        </span>
      </div>
      <ul className="flex flex-1 flex-col gap-0.5">
        {SCREENS.map((s) => {
          const Icon = ICONS[s];
          const on = screen === s;
          return (
            <li key={s}>
              <button
                type="button"
                aria-current={on ? 'page' : undefined}
                title={t(`nav.${s}`)}
                onClick={() => setScreen(s)}
                className={cn(
                  'flex h-9 w-full items-center gap-3 rounded-control px-2.5 text-sm font-medium transition-colors',
                  on
                    ? 'bg-accent-soft text-accent-text'
                    : 'text-muted hover:bg-surface-2 hover:text-fg',
                )}
              >
                <Icon className="size-4 shrink-0" aria-hidden />
                <span className="max-[899px]:sr-only">{t(`nav.${s}`)}</span>
              </button>
            </li>
          );
        })}
      </ul>
      <div className="mt-4 px-2 text-xs text-subtle">
        <div className="flex items-center gap-2">
          <span
            aria-hidden
            className={cn(
              'size-2 shrink-0 rounded-full',
              status === 'connected'
                ? 'bg-success'
                : status === 'connecting'
                  ? 'bg-warning'
                  : 'bg-danger',
            )}
          />
          <span className="max-[899px]:sr-only">{t(`connection.status.${status}`)}</span>
        </div>
        {info.data ? (
          <p className="mt-1 tabular-nums max-[899px]:hidden">v{info.data.version}</p>
        ) : null}
      </div>
    </nav>
  );
}

function TopBar() {
  const { t } = useI18n();
  const setOpen = useNav((s) => s.setPaletteOpen);
  return (
    <div className="flex h-12 shrink-0 items-center justify-end border-b border-line px-6">
      <button
        type="button"
        onClick={() => setOpen(true)}
        aria-keyshortcuts="Control+K Meta+K"
        className="flex h-8 items-center gap-2 rounded-control border border-line-strong bg-surface px-2.5 text-xs text-muted hover:bg-surface-2"
      >
        <Search className="size-3.5" aria-hidden />
        <span>{t('palette.open')}</span>
        <Kbd>{isMac() ? '⌘K' : 'Ctrl K'}</Kbd>
      </button>
    </div>
  );
}

export function Shell({ children }: { children: ReactNode }) {
  const { t } = useI18n();
  const screen = useNav((s) => s.screen);
  // Each screen starts at the top (the scroll container is shared).
  useEffect(() => {
    document.getElementById('main')?.scrollTo({ top: 0 });
  }, [screen]);
  return (
    <div className="flex h-full">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-[70] focus:rounded-control focus:bg-surface focus:px-3 focus:py-2 focus:text-sm focus:shadow-lg"
        onClick={(e) => {
          e.preventDefault();
          document.getElementById('main')?.focus();
        }}
      >
        {t('a11y.skip')}
      </a>
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col">
        <TopBar />
        <main id="main" tabIndex={-1} className="min-h-0 flex-1 overflow-y-auto outline-none">
          <div className="mx-auto max-w-5xl p-4 min-[900px]:p-6">{children}</div>
        </main>
      </div>
    </div>
  );
}
