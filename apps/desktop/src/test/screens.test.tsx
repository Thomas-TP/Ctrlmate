import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ConnectionScreen } from '../components/ConnectionState';
import { Cooling } from '../screens/Cooling';
import { HomeAssistant } from '../screens/HomeAssistant';
import { Lighting } from '../screens/Lighting';
import { Overview } from '../screens/Overview';
import { System } from '../screens/System';
import { useDaemon } from '../store/daemon';
import { makeSnapshot, renderWithApp } from './utils';

interface IpcBody {
  action?: string;
  entity_id?: string;
  preset?: string;
  [k: string]: unknown;
}

let commands: IpcBody[] = [];
let ipcCalls: { method: string; path: string }[] = [];
let commandResult: { ok: boolean; code: string; message?: string } = { ok: true, code: 'ok' };

beforeEach(() => {
  commands = [];
  ipcCalls = [];
  commandResult = { ok: true, code: 'ok' };
  mockIPC((cmd, args) => {
    const a = (args ?? {}) as { method?: string; path?: string; body?: IpcBody };
    if (cmd === 'ipc_request') {
      ipcCalls.push({ method: a.method ?? '', path: a.path ?? '' });
      if (a.path === '/ipc/command') {
        if (a.body) commands.push(a.body);
        return { status: 200, body: commandResult };
      }
      return { status: 200, body: makeSnapshot('default') };
    }
    if (cmd === 'service_status') {
      return {
        platform: 'windows',
        app_version: '0.6.0',
        daemon_version: null,
        running_as_service: null,
        agent_connected: null,
        state: 'offline',
        installer_available: true,
        agent_binary_available: true,
      };
    }
    return null;
  });
});

afterEach(() => {
  clearMocks();
});

describe('Cooling screen', () => {
  it('renders the real policy from the snapshot and sends the chosen preset', async () => {
    const user = userEvent.setup();
    const { snapshot } = renderWithApp(<Cooling snapshot={makeSnapshot('deep')} />, {
      snapshot: 'deep',
    });
    // Five presets, "Auto" selected (requested preset of the snapshot).
    const radios = within(screen.getByRole('radiogroup', { name: 'Preset' })).getAllByRole('radio');
    expect(radios.map((r) => r.id)).toEqual([
      'preset-auto',
      'preset-silent',
      'preset-balanced',
      'preset-performance',
      'preset-gaming',
    ]);
    expect(screen.getByRole('radio', { name: /Auto/ })).toHaveAttribute('aria-checked', 'true');
    // Pump floors come from the snapshot (Gaming = 90 %), not from the UI.
    const gaming = snapshot.cooling_policy?.presets.find((p) => p.preset === 'gaming');
    expect(screen.getByRole('radio', { name: /Gaming/ })).toHaveTextContent(
      `Pump ≥ ${gaming?.pump_floor}%`,
    );
    // The chart has a text alternative generated from the same points.
    expect(screen.getByRole('img', { name: /Fan curve/ })).toBeInTheDocument();
    expect(screen.getByRole('table')).toBeInTheDocument();

    await user.click(screen.getByRole('radio', { name: /Silent/ }));
    await waitFor(() => expect(commands).toHaveLength(1));
    expect(commands[0]).toEqual({
      action: 'set_cooling_preset',
      entity_id: 'cooling_preset',
      preset: 'silent',
    });
    // Cooling is never optimistic: the store still holds the daemon's value.
    expect(useDaemon.getState().snapshot?.cooling_preset).toBe('auto');
  });

  it('shows the unavailable CPU sensor with the plain-language reason and the read-only limits', () => {
    renderWithApp(<Cooling snapshot={makeSnapshot('default')} />, { snapshot: 'default' });
    expect(screen.getByText('CPU temperature is unavailable')).toBeInTheDocument();
    expect(screen.getByText(/needs a kernel driver/)).toBeInTheDocument();
    expect(screen.getByText('93°C')).toBeInTheDocument();
    expect(screen.getByText(/cannot be edited from the app/)).toBeInTheDocument();
  });

  it('announces the failsafe state with its reason', () => {
    renderWithApp(<Cooling snapshot={makeSnapshot('failsafe')} />, { snapshot: 'failsafe' });
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('Safety mode is active');
    expect(alert).toHaveTextContent('A temperature reached its critical limit.');
  });
});

describe('System screen', () => {
  it('asks for confirmation before a destructive power action, and cancel sends nothing', async () => {
    const user = userEvent.setup();
    renderWithApp(<System snapshot={makeSnapshot('default')} />);
    await user.click(screen.getByRole('button', { name: 'Shut down' }));
    const dialog = screen.getByRole('alertdialog');
    expect(within(dialog).getByText('Shut down this PC?')).toBeInTheDocument();
    expect(commands).toHaveLength(0);
    // Focus starts on Cancel so Enter can never fire the action.
    expect(within(dialog).getByRole('button', { name: 'Cancel' })).toHaveFocus();
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(commands).toHaveLength(0);

    await user.click(screen.getByRole('button', { name: 'Shut down' }));
    await user.click(
      within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Shut down' }),
    );
    await waitFor(() => expect(commands).toHaveLength(1));
    expect(commands[0]).toEqual({ action: 'press_button', entity_id: 'power_shutdown' });
  });

  it('does not confirm for lock, and explains no_agent in plain words', async () => {
    const user = userEvent.setup();
    commandResult = { ok: false, code: 'no_agent' };
    renderWithApp(<System snapshot={makeSnapshot('noagent')} />, { snapshot: 'noagent' });
    await user.click(screen.getByRole('button', { name: 'Lock' }));
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    await waitFor(() => expect(commands).toHaveLength(1));
    expect(commands[0]).toEqual({ action: 'press_button', entity_id: 'power_lock' });
    expect(
      await screen.findByText(/^Lock: The Ctrlmate agent is not running in your session/),
    ).toBeInTheDocument();
  });

  it('shows the OS update state honestly: titles, KB ids and pending count', () => {
    renderWithApp(<System snapshot={makeSnapshot('default')} />);
    expect(screen.getByText('2 updates pending')).toBeInTheDocument();
    expect(screen.getByText('KB5050009')).toBeInTheDocument();
    const fresh = makeSnapshot('default');
    fresh.updates = {
      ...fresh.updates,
      checked: false,
      pending_count: null,
      pending: [],
      titles: [],
      kb_ids: [],
      last_checked: null,
    };
    renderWithApp(<System snapshot={fresh} />, { snapshot: fresh });
    expect(screen.getByText('Not checked yet')).toBeInTheDocument();
  });

  it('refuses to save a script with a relative path', async () => {
    const user = userEvent.setup();
    renderWithApp(<System snapshot={makeSnapshot('default')} />);
    await user.click(screen.getByRole('button', { name: 'Add script' }));
    expect(screen.getAllByText('Enter the full path of the program.').length).toBeGreaterThan(0);
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });
});

describe('Home Assistant pairing', () => {
  it('shows the PIN large, the countdown and the fingerprint in groups', async () => {
    const user = userEvent.setup();
    const snap = makeSnapshot('default');
    snap.pairing = {
      ...snap.pairing,
      active: true,
      pin: '482913',
      expires_at: new Date(Date.now() + 4 * 60_000 + 30_000).toISOString(),
    };
    renderWithApp(<HomeAssistant snapshot={snap} />, { snapshot: snap });
    const pin = screen.getByRole('status', { name: 'Pairing code: 4 8 2 9 1 3' });
    expect(pin).toHaveTextContent('482 913');
    expect(screen.getByText(/0[34]:[0-5]\d/)).toBeInTheDocument();
    // 64 hex characters in 16 groups of 4.
    expect(screen.getByLabelText(/Certificate fingerprint: A3F1 C2D4/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Cancel pairing' }));
    await waitFor(() => expect(ipcCalls.some((c) => c.path === '/ipc/pairing/cancel')).toBe(true));
  });

  it('offers a new session once the code expired', () => {
    const snap = makeSnapshot('default');
    snap.pairing = {
      ...snap.pairing,
      active: true,
      pin: '111111',
      expires_at: new Date(Date.now() - 1000).toISOString(),
    };
    renderWithApp(<HomeAssistant snapshot={snap} />, { snapshot: snap });
    expect(screen.queryByText('111 111')).not.toBeInTheDocument();
    expect(screen.getByText(/pairing code expired/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start pairing' })).toBeInTheDocument();
  });

  it('warns on the custom scripts permission and sends the whole permission set', async () => {
    const user = userEvent.setup();
    renderWithApp(<HomeAssistant snapshot={makeSnapshot('default')} />);
    expect(screen.getByText(/Scripts run as your user/)).toBeInTheDocument();
    const sw = screen.getByRole('switch', { name: 'Run custom scripts' });
    expect(sw).toHaveAttribute('aria-checked', 'false');
    await user.click(sw);
    await waitFor(() =>
      expect(ipcCalls.some((c) => c.method === 'PUT' && c.path === '/ipc/settings')).toBe(true),
    );
  });
});

describe('Lighting screen', () => {
  it('renders exactly the zones of the snapshot with capability-driven controls', () => {
    renderWithApp(<Lighting snapshot={makeSnapshot('default')} />);
    const headings = screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent);
    expect(headings).toEqual([
      'All lights · Master',
      'Motherboard',
      'Memory',
      'Fans',
      'Pump head',
      'Detected but not controllable',
    ]);
    const kelvin = screen.getAllByLabelText('Colour temperature')[0];
    expect(kelvin).toHaveAttribute('min', '2000');
    expect(kelvin).toHaveAttribute('max', '6500');
    // No effect modes exist in the protocol, so none are offered.
    expect(screen.queryByText(/effect|rainbow|breathing/i)).not.toBeInTheDocument();
    // The Samsung LED has no zone: listed as informational with the translated reason.
    expect(screen.getByText('Samsung 990 PRO LED')).toBeInTheDocument();
    expect(screen.getByText(/No public protocol exists/)).toBeInTheDocument();
  });

  it('sends a power toggle for the right zone', async () => {
    const user = userEvent.setup();
    renderWithApp(<Lighting snapshot={makeSnapshot('default')} />);
    const card = screen.getByRole('heading', { name: 'Pump head' }).closest('section');
    if (!card) throw new Error('zone card not found');
    await user.click(within(card).getByRole('switch', { name: 'Power' }));
    await waitFor(() => expect(commands).toHaveLength(1));
    expect(commands[0]).toMatchObject({
      action: 'set_light',
      entity_id: 'light_cooler',
      is_on: true,
    });
  });

  it('shows an explicit empty state when no zone exists', () => {
    const snap = makeSnapshot('default');
    snap.manifest.entities = snap.manifest.entities.filter((e) => e.platform !== 'light');
    snap.rgb_devices = [];
    renderWithApp(<Lighting snapshot={snap} />, { snapshot: snap });
    expect(screen.getByText('No lighting zone detected')).toBeInTheDocument();
  });
});

describe('Overview and connection states', () => {
  it('shows the translated reason instead of a fake CPU temperature (French)', () => {
    renderWithApp(<Overview snapshot={makeSnapshot('default')} />, { language: 'fr' });
    const card = screen.getByRole('region', { name: 'Température CPU' });
    expect(card).toHaveTextContent('Indisponible');
    expect(card).toHaveTextContent('Nécessite le mode matériel approfondi');
  });

  it('formats temperatures in Fahrenheit through the single formatter', () => {
    const snap = makeSnapshot('deep');
    snap.temperature_unit = 'fahrenheit';
    renderWithApp(<Overview snapshot={snap} />, { snapshot: snap });
    expect(screen.getByRole('region', { name: 'GPU temperature' })).toHaveTextContent('115°F');
  });

  it('explains an unreadable token on Linux with the exact fix', async () => {
    renderWithApp(<ConnectionScreen />, { snapshot: 'default' });
    act(() => {
      useDaemon.setState({ snapshot: null });
      useDaemon.getState().applyConnection({
        status: 'unauthorized',
        reason: 'token_permission_denied',
        attempt: 1,
        retry_in_ms: 2000,
      });
    });
    expect(await screen.findByText(/sudo usermod -aG ctrlmate/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry now' })).toBeInTheDocument();
  });
});
