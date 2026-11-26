import '@fontsource-variable/inter/wght.css';
import '@fontsource-variable/jetbrains-mono/wght.css';
import './index.css';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { applyThemeEarly } from './hooks/useTheme';
import { usePrefs } from './store/prefs';

async function boot(): Promise<void> {
  // Dev-only mock daemon + Tauri bridge. `import.meta.env.DEV` is false in production builds, so
  // this branch and the whole `dev/` folder are removed by the bundler.
  if (import.meta.env.DEV && new URLSearchParams(window.location.search).has('mock')) {
    const { installMockBridge } = await import('../dev/mock-bridge');
    installMockBridge(new URLSearchParams(window.location.search).get('mock') ?? '1');
  }
  applyThemeEarly(usePrefs.getState().theme);
  const root = document.getElementById('root');
  if (!root) throw new Error('missing #root');
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void boot();
