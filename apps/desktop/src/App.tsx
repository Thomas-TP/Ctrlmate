import { CommandPalette } from './components/CommandPalette';
import { ConnectionBanner, ConnectionScreen } from './components/ConnectionState';
import { ErrorBoundary } from './components/ErrorBoundary';
import { Shell } from './components/Shell';
import { Toasts } from './components/ui/Toasts';
import { useTheme, useTrayLabels } from './hooks/useTheme';
import { I18nProvider } from './i18n';
import { Cooling } from './screens/Cooling';
import { HomeAssistant } from './screens/HomeAssistant';
import { Lighting } from './screens/Lighting';
import { Overview } from './screens/Overview';
import { Settings } from './screens/Settings';
import { System } from './screens/System';
import { useDaemonBridge } from './store/bridge';
import { useDaemon } from './store/daemon';
import { useNav } from './store/nav';

function Screens() {
  const screen = useNav((s) => s.screen);
  const snapshot = useDaemon((s) => s.snapshot);
  useTrayLabels();

  // Settings stays reachable without a daemon (language, theme, service install).
  if (screen === 'settings') return <Settings snapshot={snapshot} />;
  if (!snapshot) return <ConnectionScreen />;
  return (
    <>
      <ConnectionBanner />
      {screen === 'overview' && <Overview snapshot={snapshot} />}
      {screen === 'cooling' && <Cooling snapshot={snapshot} />}
      {screen === 'lighting' && <Lighting snapshot={snapshot} />}
      {screen === 'system' && <System snapshot={snapshot} />}
      {screen === 'homeAssistant' && <HomeAssistant snapshot={snapshot} />}
    </>
  );
}

function Frame() {
  const screen = useNav((s) => s.screen);
  useTheme();
  useDaemonBridge();
  return (
    <Shell>
      <ErrorBoundary resetKey={screen}>
        <Screens />
      </ErrorBoundary>
      <CommandPalette />
      <Toasts />
    </Shell>
  );
}

export default function App() {
  return (
    <I18nProvider>
      <Frame />
    </I18nProvider>
  );
}
