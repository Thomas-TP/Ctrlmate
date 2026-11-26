import { create } from 'zustand';
import type { ConnectionStatus } from '../api/app';
import type { DaemonSnapshot } from '../types/protocol';
import { initialView, reduceConnection, reduceSnapshot, type DaemonView } from './reducers';

export type ToastKind = 'success' | 'error' | 'info';
export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  detail?: string;
}

interface DaemonStore extends DaemonView {
  toasts: Toast[];
  applySnapshot: (s: DaemonSnapshot) => void;
  applyConnection: (c: ConnectionStatus) => void;
  pushToast: (kind: ToastKind, message: string, detail?: string) => void;
  dismissToast: (id: number) => void;
  /** Test helper / reset. */
  reset: () => void;
}

let toastSeq = 0;

export const useDaemon = create<DaemonStore>()((set) => ({
  ...initialView(Date.now()),
  toasts: [],
  applySnapshot: (s) => set((v) => reduceSnapshot(v, s, Date.now())),
  applyConnection: (c) => set((v) => reduceConnection(v, c, Date.now())),
  pushToast: (kind, message, detail) =>
    set((v) => ({
      toasts: [...v.toasts.slice(-3), { id: ++toastSeq, kind, message, detail }],
    })),
  dismissToast: (id) => set((v) => ({ toasts: v.toasts.filter((t) => t.id !== id) })),
  reset: () => set({ ...initialView(Date.now()), toasts: [] }),
}));

/** Snapshot that is guaranteed present: screens render this only behind the connection gate. */
export function useSnapshot(): DaemonSnapshot | null {
  return useDaemon((s) => s.snapshot);
}
