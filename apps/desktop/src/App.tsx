import { useEffect } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { app } from './api/app';
import { useTheme } from './hooks/useTheme';
import { I18nProvider, useI18n } from './i18n';
import { useDaemonBridge } from './store/bridge';
import { ErrorBoundary } from './ui/ErrorBoundary';
import { MainView } from './views/main/MainView';
import { TrayView } from './views/tray/TrayView';

/**
 * The same bundle serves two windows: the quick panel (`?view=tray`, or the Tauri window labelled
 * `tray`) and the app itself.
 */
export function isTrayView(): boolean {
  if (new URLSearchParams(window.location.search).get('view') === 'tray') return true;
  try {
    return '__TAURI_INTERNALS__' in window && getCurrentWindow().label === 'tray';
  } catch {
    return false;
  }
}

/** Pushes the translated tray menu labels to the Rust side (language changes included). */
function useTrayLabels(enabled: boolean): void {
  const { t, locale } = useI18n();
  useEffect(() => {
    if (!enabled) return;
    app.setTrayLabels(t('tray.panel'), t('tray.show'), t('tray.quit')).catch(() => {
      /* not running inside Tauri */
    });
  }, [t, locale, enabled]);
}

const TRAY = isTrayView();

function Frame() {
  useTheme(!TRAY);
  useDaemonBridge();
  useTrayLabels(!TRAY);
  return <ErrorBoundary>{TRAY ? <TrayView /> : <MainView />}</ErrorBoundary>;
}

export default function App() {
  return (
    <I18nProvider>
      <Frame />
    </I18nProvider>
  );
}
