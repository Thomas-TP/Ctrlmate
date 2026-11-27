import { useCallback, useState } from 'react';
import { app } from '../api/app';
import { useServiceStatus, type Resource } from './hooks';
import type { ServiceStatus } from '../api/app';

export type SetupPhase = 'idle' | 'working' | 'cancelled' | 'failed' | 'manual' | 'done';

export interface ServiceSetup {
  status: Resource<ServiceStatus>;
  phase: SetupPhase;
  /** One elevated step (one UAC prompt) that installs or upgrades the service, then reconnects. */
  run: (action: 'install' | 'upgrade') => Promise<SetupPhase>;
}

/** Shared by the onboarding, the connection screen and the settings. */
export function useServiceSetup(): ServiceSetup {
  const status = useServiceStatus();
  const { reload } = status;
  const [phase, setPhase] = useState<SetupPhase>('idle');

  const run = useCallback(
    async (action: 'install' | 'upgrade'): Promise<SetupPhase> => {
      setPhase('working');
      let next: SetupPhase = 'failed';
      try {
        const out = await app.serviceInstallOrUpgrade(action);
        if (out.status === 'completed') {
          // The session agent needs its own autostart entry; best effort, the service works without.
          await app.agentInstallAutostart().catch(() => null);
          await app.reconnect().catch(() => null);
          next = 'done';
        } else if (out.status === 'cancelled') next = 'cancelled';
        else if (out.status === 'manual') next = 'manual';
      } catch {
        next = 'failed';
      }
      setPhase(next);
      reload();
      return next;
    },
    [reload],
  );

  return { status, phase, run };
}
