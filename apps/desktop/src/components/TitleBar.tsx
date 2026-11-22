import { useEffect, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Minus, Moon, Search, ShieldCheck, Square, Sun, X } from 'lucide-react';
import type { SecurityMode, TemperatureUnit } from '../types';
import { CtrlLogo } from './Primitives';

function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export function TitleBar({
  version,
  hostName,
  securityMode,
  unit,
  dark,
  onToggleUnit,
  onToggleTheme,
  onOpenPalette,
}: {
  version?: string;
  hostName?: string;
  securityMode?: SecurityMode;
  unit: TemperatureUnit;
  dark: boolean;
  onToggleUnit: () => void;
  onToggleTheme: () => void;
  onOpenPalette: () => void;
}) {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    const win = getCurrentWindow();
    win.isMaximized().then(setMaximized).catch(() => {});
    const unlistenPromise = win.onResized(() => {
      win.isMaximized().then(setMaximized).catch(() => {});
    });
    return () => {
      unlistenPromise.then((fn) => fn()).catch(() => {});
    };
  }, []);

  const handleMinimize = () => {
    if (isTauriRuntime()) getCurrentWindow().minimize().catch(() => {});
  };

  const handleToggleMaximize = () => {
    if (isTauriRuntime()) getCurrentWindow().toggleMaximize().catch(() => {});
  };

  const handleClose = () => {
    if (isTauriRuntime()) getCurrentWindow().close().catch(() => {});
  };

  return (
    <header
      data-tauri-drag-region
      onDoubleClick={(e) => {
        if ((e.target as HTMLElement).closest('button')) return;
        handleToggleMaximize();
      }}
      className="hairline-b flex h-10 shrink-0 items-center justify-between bg-[var(--bg-chrome)] pl-3.5 select-none"
    >
      {/* Left: Brand identity */}
      <div className="flex items-center gap-2.5" data-tauri-drag-region>
        <div className="flex h-6 w-6 items-center justify-center rounded-md bg-neutral-900 text-white dark:bg-white dark:text-neutral-950">
          <CtrlLogo className="h-3.5 w-3.5" />
        </div>
        <div className="flex items-baseline gap-2" data-tauri-drag-region>
          <span className="text-[13px] font-semibold tracking-[-0.02em]">Ctrlmate</span>
          {version && (
            <span className="tabular text-[10px] text-[var(--text-muted)]">v{version}</span>
          )}
          {hostName && (
            <span className="hidden text-[11px] text-[var(--text-muted)] sm:inline" data-tauri-drag-region>
              · {hostName}
            </span>
          )}
        </div>
      </div>

      {/* Center: Raycast / Linear-style Command Palette trigger */}
      <button
        type="button"
        onClick={onOpenPalette}
        className="flex h-7 w-56 items-center justify-between rounded-md bg-black/[0.04] px-2.5 text-[11px] text-[var(--text-muted)] ring-1 ring-black/[0.06] transition hover:bg-black/[0.07] hover:text-[var(--text-primary)] sm:w-72 dark:bg-white/[0.04] dark:ring-white/[0.07] dark:hover:bg-white/[0.08]"
      >
        <span className="flex items-center gap-2 truncate">
          <Search className="h-3 w-3 shrink-0" />
          <span className="truncate">Commandes rapides…</span>
        </span>
        <kbd className="tabular rounded bg-black/[0.06] px-1.5 py-0.5 text-[10px] text-[var(--text-muted)] dark:bg-white/[0.08]">
          Ctrl K
        </kbd>
      </button>

      {/* Right: Quick pills + Native window chrome */}
      <div className="flex h-full items-center">
        <div className="mr-2 flex items-center gap-1.5">
          {securityMode && (
            <span
              title={
                securityMode === 'zero_kernel_safe'
                  ? 'Mode Zero-Kernel / Anti-Cheat Safe (aucun pilote noyau)'
                  : 'Mode Deep Hardware (PawnIO avec auto-pause FACEIT)'
              }
              className="hidden items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium text-[var(--text-secondary)] ring-1 ring-black/[0.06] md:inline-flex dark:ring-white/[0.08]"
            >
              <ShieldCheck
                className={`h-3 w-3 ${
                  securityMode === 'zero_kernel_safe'
                    ? 'text-emerald-500'
                    : 'text-amber-500'
                }`}
              />
              <span>
                {securityMode === 'zero_kernel_safe' ? 'Anti-Cheat Safe' : 'Deep PawnIO'}
              </span>
            </span>
          )}

          <button
            type="button"
            onClick={onToggleUnit}
            title="Basculer °C / °F"
            className="tabular rounded-md px-2 py-1 text-[11px] font-medium text-[var(--text-secondary)] ring-1 ring-black/[0.06] transition hover:bg-black/[0.05] hover:text-[var(--text-primary)] dark:ring-white/[0.08] dark:hover:bg-white/[0.07]"
          >
            {unit === 'celsius' ? '°C' : '°F'}
          </button>

          <button
            type="button"
            onClick={onToggleTheme}
            title={dark ? 'Passer en thème clair' : 'Passer en thème sombre'}
            className="flex h-6 w-6 items-center justify-center rounded-md text-[var(--text-secondary)] transition hover:bg-black/[0.05] hover:text-[var(--text-primary)] dark:hover:bg-white/[0.07]"
          >
            {dark ? <Sun className="h-3.5 w-3.5" /> : <Moon className="h-3.5 w-3.5" />}
          </button>
        </div>

        {/* Minimalist Windows 11-grade frameless controls */}
        <div className="flex h-full items-center border-l border-[var(--border-hairline)]">
          <button
            type="button"
            onClick={handleMinimize}
            title="Réduire"
            className="flex h-full w-11 items-center justify-center text-[var(--text-secondary)] transition hover:bg-black/[0.05] hover:text-[var(--text-primary)] dark:hover:bg-white/[0.07]"
          >
            <Minus className="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            onClick={handleToggleMaximize}
            title={maximized ? 'Restaurer' : 'Agrandir'}
            className="flex h-full w-11 items-center justify-center text-[var(--text-secondary)] transition hover:bg-black/[0.05] hover:text-[var(--text-primary)] dark:hover:bg-white/[0.07]"
          >
            <Square className="h-3 w-3" />
          </button>
          <button
            type="button"
            onClick={handleClose}
            title="Fermer"
            className="flex h-full w-11 items-center justify-center text-[var(--text-secondary)] transition hover:bg-red-600 hover:text-white"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>
    </header>
  );
}
