import { render } from '@testing-library/react';
import type { ReactElement } from 'react';
import { makeSnapshot, type Scenario } from '../../dev/mock-daemon';
import { I18nProvider } from '../i18n';
import { useDaemon } from '../store/daemon';
import { useNav } from '../store/nav';
import { usePrefs } from '../store/prefs';
import type { DaemonSnapshot } from '../types/protocol';

export { makeSnapshot };

/** Renders `ui` inside the i18n provider with the daemon store seeded from a snapshot. */
export function renderWithApp(
  ui: ReactElement,
  opts: { snapshot?: DaemonSnapshot | Scenario; language?: 'en' | 'fr' } = {},
) {
  usePrefs.setState({ language: opts.language ?? 'en', theme: 'light' });
  useNav.setState({ screen: 'overview', paletteOpen: false });
  useDaemon.getState().reset();
  const snap =
    typeof opts.snapshot === 'object' ? opts.snapshot : makeSnapshot(opts.snapshot ?? 'default');
  useDaemon.getState().applySnapshot(snap);
  useDaemon.getState().applyConnection({ status: 'connected', attempt: 0 });
  return { snapshot: snap, ...render(<I18nProvider>{ui}</I18nProvider>) };
}
