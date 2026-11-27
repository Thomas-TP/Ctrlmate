import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { useDaemon } from '../store/daemon';
import { useUi } from '../store/ui';
import { Onboarding } from '../views/onboarding/Onboarding';
import { MainView } from '../views/main/MainView';
import { TrayView } from '../views/tray/TrayView';
import { makeSnapshot, renderWithApp } from './utils';

interface IpcBody {
  action?: string;
  entity_id?: string;
  preset?: string;
  [k: string]: unknown;
}

let commands: IpcBody[] = [];
let tauriCalls: string[] = [];

beforeEach(() => {
  commands = [];
  tauriCalls = [];
  mockIPC((cmd, args) => {
    tauriCalls.push(cmd);
    const a = (args ?? {}) as { path?: string; body?: IpcBody };
    if (cmd === 'ipc_request') {
      if (a.path === '/ipc/command') {
        if (a.body) commands.push(a.body);
        return { status: 200, body: { ok: true, code: 'ok' } };
      }
      return { status: 200, body: makeSnapshot('default') };
    }
    if (cmd === 'service_status') {
      return {
        platform: 'windows',
        app_version: '0.7.0',
        daemon_version: null,
        running_as_service: null,
        agent_connected: null,
        state: 'offline',
        installer_available: true,
        agent_binary_available: true,
      };
    }
    if (cmd === 'app_update_check') {
      return {
        state: 'up_to_date',
        channel: 'win',
        current_version: '0.7.0',
        latest_version: null,
        detail: null,
      };
    }
    if (cmd === 'app_settings_get') {
      return { close_to_tray: true, start_hidden: false, launch_at_login: true };
    }
    return null;
  });
});

afterEach(() => {
  clearMocks();
});

describe('dashboard', () => {
  it('reassures when everything is fine and shows the honest sensor state', async () => {
    renderWithApp(<MainView />);
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Everything is under control' }),
    ).toBeInTheDocument();
    // CPU temperature needs deep hardware in the default scenario: never a made-up number.
    expect(screen.getAllByText('Unavailable').length).toBeGreaterThan(0);
  });

  it('puts the safety mode in the headline and as an alert', async () => {
    renderWithApp(<MainView />, { snapshot: 'failsafe' });
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Safety mode is active' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('Cooling safety mode is active');
  });

  it('sends the chosen cooling preset to the daemon, straight from the tile', async () => {
    const user = userEvent.setup();
    renderWithApp(<MainView />);
    const group = await screen.findByRole('radiogroup', { name: 'Preset' });
    await user.click(within(group).getByRole('radio', { name: 'Performance' }));
    await waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ action: 'set_cooling_preset', preset: 'performance' }),
      ),
    );
  });

  it('opens a detail panel when a tile is clicked, and closes it with the close button', async () => {
    const user = userEvent.setup();
    renderWithApp(<MainView />);
    await user.click(await screen.findByRole('button', { name: /^Cooling/ }));
    expect(useUi.getState().sheet).toBe('cooling');
    const dialog = await screen.findByRole('dialog', { name: 'Cooling', hidden: true });
    expect(within(dialog).getByText('Safety limits')).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Close', hidden: true }));
    expect(useUi.getState().sheet).toBeNull();
  });

  it('opens the command palette with Ctrl+K and filters commands', async () => {
    const user = userEvent.setup();
    renderWithApp(<MainView />);
    await screen.findByRole('heading', { level: 1 });
    await user.keyboard('{Control>}k{/Control}');
    const input = await screen.findByRole('combobox', { hidden: true });
    await user.type(input, 'silent');
    const options = await screen.findAllByRole('option', { hidden: true });
    expect(options.length).toBeGreaterThan(0);
    expect(options[0]).toHaveTextContent(/Silent/i);
    await user.keyboard('{Enter}');
    await waitFor(() =>
      expect(commands).toContainEqual(
        expect.objectContaining({ action: 'set_cooling_preset', preset: 'silent' }),
      ),
    );
  });

  it('explains a missing service and offers the repair instead of an empty screen', async () => {
    renderWithApp(<MainView />, { snapshot: null });
    act(() => {
      useDaemon.getState().applyConnection({
        status: 'offline',
        reason: 'daemon_offline',
        attempt: 1,
        retry_in_ms: 4000,
      });
    });
    expect(await screen.findByRole('button', { name: /Retry now/i })).toBeInTheDocument();
  });
});

describe('onboarding', () => {
  it('cannot go past the install step until the service answers', async () => {
    renderWithApp(<Onboarding />, { snapshot: null, onboarded: false });
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Welcome to Ctrlmate' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
    expect(await screen.findByRole('button', { name: /Install and continue/ })).toBeInTheDocument();
  });

  it('continues once connected and shows hardware, cooling safety check and Home Assistant', async () => {
    const user = userEvent.setup();
    renderWithApp(<Onboarding />, { snapshot: 'deep', onboarded: false });
    expect(await screen.findByText('Everything is installed')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Continue' }));
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Your hardware' }),
    ).toBeInTheDocument();
    expect(screen.getByText('NVIDIA GeForce RTX 5070')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Continue' }));
    expect(await screen.findByText('Thermal safety check')).toBeInTheDocument();
    expect(screen.getByText('All checks passed')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Continue' }));
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Home Assistant' }),
    ).toBeInTheDocument();
  });
});

describe('tray panel', () => {
  it('shows the essentials and opens the app', async () => {
    const user = userEvent.setup();
    renderWithApp(<TrayView />);
    expect(await screen.findByRole('radiogroup', { name: 'Preset' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /Open Ctrlmate/ }));
    await waitFor(() => expect(tauriCalls).toContain('open_main'));
  });
});
