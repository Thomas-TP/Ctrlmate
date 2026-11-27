import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** Codes produced by the Tauri side (`src-tauri/src/error.rs`). Rust never sends sentences. */
export const BRIDGE_ERROR_CODES = [
  'route_not_allowed',
  'bad_request',
  'token_not_found',
  'token_permission_denied',
  'token_invalid',
  'unauthorized',
  'daemon_offline',
  'daemon_protocol',
  'not_supported',
  'binary_not_found',
  'nothing_to_do',
  'internal',
] as const;
export type BridgeErrorCode = (typeof BRIDGE_ERROR_CODES)[number];

export class BridgeError extends Error {
  readonly code: string;
  readonly detail: string | undefined;
  constructor(code: string, detail?: string) {
    super(code);
    this.name = 'BridgeError';
    this.code = code;
    this.detail = detail;
  }
}

function toBridgeError(e: unknown): BridgeError {
  if (e instanceof BridgeError) return e;
  if (typeof e === 'object' && e !== null && 'code' in e) {
    const o = e as { code: unknown; detail?: unknown };
    if (typeof o.code === 'string') {
      return new BridgeError(o.code, typeof o.detail === 'string' ? o.detail : undefined);
    }
  }
  return new BridgeError('internal', e instanceof Error ? e.message : String(e));
}

/** Typed wrapper over Tauri `invoke`: every failure becomes a `BridgeError` with a code. */
export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toBridgeError(e);
  }
}

// The payload type is asserted on purpose: each caller knows the shape of its event.
// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
export async function on<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  const off = await listen<T>(event, (e) => handler(e.payload));
  // Unlistening can fail when the window is already gone: that must never surface as an error.
  return () => {
    try {
      void Promise.resolve(off()).catch(() => undefined);
    } catch {
      /* nothing left to unregister */
    }
  };
}

export const EVENT_DAEMON = 'daemon://event';
export const EVENT_CONNECTION = 'daemon://connection';
export const EVENT_UPDATE_PROGRESS = 'app-update://progress';
/** The tray panel asks the main window to open a section (payload: a `SheetId`). */
export const EVENT_OPEN_SHEET = 'ui://open';
