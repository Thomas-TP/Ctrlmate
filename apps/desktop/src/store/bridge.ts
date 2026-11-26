import { useEffect } from 'react';
import { app, listenConnection, listenDaemon } from '../api/app';
import { daemon } from '../api/daemon';
import { useDaemon } from './daemon';

/**
 * Connects the store to the Tauri side: daemon events, connection status, and an initial
 * snapshot fetch. Everything the daemon sends reaches the webview through Rust.
 */
export function useDaemonBridge(): void {
  useEffect(() => {
    const live = { disposed: false };
    const unlisten: (() => void)[] = [];
    const { applySnapshot, applyConnection } = useDaemon.getState();

    const fetchSnapshot = () => {
      daemon
        .snapshot()
        .then((s) => {
          if (!live.disposed) applySnapshot(s);
        })
        .catch(() => {
          /* the connection status event explains why */
        });
    };

    void (async () => {
      const offs = await Promise.all([
        listenDaemon((e) => {
          if (e.type === 'snapshot') applySnapshot(e.snapshot);
        }),
        listenConnection((c) => {
          applyConnection(c);
          if (c.status === 'connected') fetchSnapshot();
        }),
      ]);
      if (live.disposed) {
        offs.forEach((off) => off());
        return;
      }
      unlisten.push(...offs);
      try {
        const status = await app.connectionStatus();
        applyConnection(status);
        if (status.status === 'connected') fetchSnapshot();
      } catch {
        /* not running inside Tauri: the connection stays "connecting" */
      }
    })();

    return () => {
      live.disposed = true;
      unlisten.forEach((off) => off());
    };
  }, []);
}
