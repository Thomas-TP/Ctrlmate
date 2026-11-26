import { useMemo } from 'react';
import { DaemonHttpError, daemon } from '../api/daemon';
import { BridgeError } from '../api/tauri';
import { useI18n } from '../i18n';
import type { TKey } from '../i18n/core';
import type {
  CommandResult,
  ControlCommand,
  DaemonSnapshot,
  SettingsUpdate,
} from '../types/protocol';
import { useDaemon } from './daemon';

export interface RunOptions {
  /** Toast shown when the command succeeded. */
  successKey?: TKey;
  /** Suppress the error toast (the caller renders the result itself). */
  silent?: boolean;
}

export interface Actions {
  /** Sends a control command. Returns the `CommandResult`, or `null` on transport failure. */
  run: (command: ControlCommand, opts?: RunOptions) => Promise<CommandResult | null>;
  /** Applies a settings update; returns the new snapshot or `null` when rejected. */
  settings: (update: SettingsUpdate, opts?: RunOptions) => Promise<DaemonSnapshot | null>;
  /** Runs a snapshot-returning call (pairing, revoke, rescan) with error toasts. */
  snapshotCall: (
    fn: () => Promise<DaemonSnapshot>,
    opts?: RunOptions,
  ) => Promise<DaemonSnapshot | null>;
}

type CodeFn = ReturnType<typeof useI18n>['code'];

/** Translated, user-facing description of an arbitrary failure. */
export function describeError(code: CodeFn, e: unknown): string {
  if (e instanceof DaemonHttpError) return code('result', e.code);
  if (e instanceof BridgeError) return code('connection.reason', e.code);
  return code('result', 'internal');
}

export function useErrorText() {
  const { code } = useI18n();
  return (e: unknown): string => describeError(code, e);
}

export function useActions(): Actions {
  const { t, code } = useI18n();

  return useMemo<Actions>(() => {
    const { applySnapshot, pushToast } = useDaemon.getState();
    const fail = (e: unknown, silent?: boolean) => {
      if (silent) return;
      pushToast('error', describeError(code, e), e instanceof BridgeError ? e.detail : undefined);
    };
    return {
      run: async (command, opts) => {
        try {
          const result = await daemon.command(command);
          if (result.ok) {
            if (opts?.successKey) pushToast('success', t(opts.successKey));
          } else if (!opts?.silent) {
            pushToast('error', code('result', result.code), result.message ?? undefined);
          }
          return result;
        } catch (e) {
          fail(e, opts?.silent);
          return null;
        }
      },
      settings: async (update, opts) => {
        try {
          const snap = await daemon.updateSettings(update);
          applySnapshot(snap);
          if (opts?.successKey) pushToast('success', t(opts.successKey));
          return snap;
        } catch (e) {
          fail(e, opts?.silent);
          return null;
        }
      },
      snapshotCall: async (fn, opts) => {
        try {
          const snap = await fn();
          applySnapshot(snap);
          if (opts?.successKey) pushToast('success', t(opts.successKey));
          return snap;
        } catch (e) {
          fail(e, opts?.silent);
          return null;
        }
      },
    };
  }, [t, code]);
}
