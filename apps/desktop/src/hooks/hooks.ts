import { useCallback, useEffect, useRef, useState } from 'react';
import { app, type AppInfo, type ServiceStatus } from '../api/app';

/** Re-renders every `ms` and returns the current epoch ms (countdowns, relative times). */
export function useNow(ms = 1000, active = true): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms, active]);
  return now;
}

export interface Resource<T> {
  data: T | null;
  error: unknown;
  loading: boolean;
  reload: () => void;
}

/** Loads a value once (and on `reload`). Unmount-safe. */
export function useResource<T>(load: () => Promise<T>): Resource<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [loading, setLoading] = useState(true);
  const fn = useRef(load);
  useEffect(() => {
    fn.current = load;
  });
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  // The state starts as `loading`, so the first fetch needs no synchronous setState in an effect.
  const fetchOnce = useCallback(() => {
    fn.current()
      .then((v) => {
        if (!alive.current) return;
        setData(v);
        setError(null);
      })
      .catch((e: unknown) => {
        if (alive.current) setError(e);
      })
      .finally(() => {
        if (alive.current) setLoading(false);
      });
  }, []);
  const reload = useCallback(() => {
    setLoading(true);
    fetchOnce();
  }, [fetchOnce]);
  useEffect(() => {
    fetchOnce();
  }, [fetchOnce]);
  return { data, error, loading, reload };
}

export const useAppInfo = (): Resource<AppInfo> => useResource(app.info);
export const useServiceStatus = (): Resource<ServiceStatus> => useResource(app.serviceStatus);

/** Debounces `fn`; the latest call wins. */
export function useDebounced<A extends unknown[]>(fn: (...a: A) => void, ms: number) {
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const latest = useRef(fn);
  useEffect(() => {
    latest.current = fn;
  });
  useEffect(() => () => clearTimeout(timer.current), []);
  return useCallback(
    (...a: A) => {
      clearTimeout(timer.current);
      timer.current = setTimeout(() => latest.current(...a), ms);
    },
    [ms],
  );
}
