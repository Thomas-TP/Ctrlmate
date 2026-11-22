import { useEffect, useMemo, useRef, useState } from 'react';
import type { DaemonSnapshot, EntityState, LightState, OsUpdateState } from '../types';

export interface TelemetryHistory {
  cpuLoad: number[];
  cpuTemp: number[];
  gpuTemp: number[];
  gpuLoad: number[];
  gpuPower: number[];
  liquidTemp: number[];
  ramUsage: number[];
  ssdTemp: number[];
}

const MAX_POINTS = 60;

function pushSample(arr: number[], value: number | undefined): number[] {
  if (value === undefined || Number.isNaN(value)) return arr;
  const next = arr.length >= MAX_POINTS ? arr.slice(arr.length - MAX_POINTS + 1) : arr.slice();
  next.push(value);
  return next;
}

export function useSnapshotHelpers(snapshot: DaemonSnapshot | null) {
  const stateMap = useMemo(() => {
    const map = new Map<string, EntityState>();
    if (snapshot) {
      for (const s of snapshot.states) {
        map.set(s.id, s);
      }
    }
    return map;
  }, [snapshot]);

  const numVal = (id: string): number | undefined => {
    const s = stateMap.get(id);
    if (!s) return undefined;
    if (s.value.kind === 'float' || s.value.kind === 'int') return s.value.value;
    return undefined;
  };

  const boolVal = (id: string): boolean => {
    const s = stateMap.get(id);
    return s?.value.kind === 'bool' ? s.value.value : false;
  };

  const textVal = (id: string): string => {
    const s = stateMap.get(id);
    return s?.value.kind === 'text' ? s.value.value : '';
  };

  const lightVal = (id: string): LightState | undefined => {
    const s = stateMap.get(id);
    return s?.value.kind === 'light' ? s.value.value : undefined;
  };

  const updateVal = (): OsUpdateState | undefined => {
    const s = stateMap.get('os_updates');
    return s?.value.kind === 'update' ? s.value.value : undefined;
  };

  const attrVal = (id: string, key: string): unknown => {
    return stateMap.get(id)?.attributes?.[key];
  };

  const primarySsd = useMemo(() => {
    if (!snapshot) return undefined;
    const storageDevs = snapshot.manifest.child_devices.filter((d) => d.category === 'storage');
    const preferred =
      storageDevs.find((d) => d.name.toLowerCase().includes('990') || d.name.toLowerCase().includes('samsung')) ??
      storageDevs.find((d) => numVal(`${d.id}_temperature`) !== undefined) ??
      storageDevs[0];
    if (!preferred) return undefined;
    return {
      id: preferred.id,
      name: preferred.name,
      temp: numVal(`${preferred.id}_temperature`),
      usage: numVal(`${preferred.id}_usage`),
    };
  }, [snapshot, stateMap]);

  return { stateMap, numVal, boolVal, textVal, lightVal, updateVal, attrVal, primarySsd };
}

export function useTelemetryHistory(snapshot: DaemonSnapshot | null): TelemetryHistory {
  const [history, setHistory] = useState<TelemetryHistory>({
    cpuLoad: [],
    cpuTemp: [],
    gpuTemp: [],
    gpuLoad: [],
    gpuPower: [],
    liquidTemp: [],
    ramUsage: [],
    ssdTemp: [],
  });

  const lastTickRef = useRef<string>('');

  useEffect(() => {
    if (!snapshot || snapshot.states.length === 0) return;
    const tickStamp = snapshot.states[0]?.updated_at ?? '';
    if (tickStamp === lastTickRef.current) return;
    lastTickRef.current = tickStamp;

    const getNum = (id: string): number | undefined => {
      const s = snapshot.states.find((x) => x.id === id);
      if (!s) return undefined;
      return s.value.kind === 'float' || s.value.kind === 'int' ? s.value.value : undefined;
    };

    const ssdTemp =
      getNum('storage_1_temperature') ?? getNum('storage_0_temperature');

    setHistory((prev) => ({
      cpuLoad: pushSample(prev.cpuLoad, getNum('cpu_0_load')),
      cpuTemp: pushSample(prev.cpuTemp, getNum('cpu_0_temperature')),
      gpuTemp: pushSample(prev.gpuTemp, getNum('gpu_0_temperature')),
      gpuLoad: pushSample(prev.gpuLoad, getNum('gpu_0_load')),
      gpuPower: pushSample(prev.gpuPower, getNum('gpu_0_power')),
      liquidTemp: pushSample(prev.liquidTemp, getNum('cooler_0_liquid_temperature')),
      ramUsage: pushSample(prev.ramUsage, getNum('memory_0_usage')),
      ssdTemp: pushSample(prev.ssdTemp, ssdTemp),
    }));
  }, [snapshot]);

  return history;
}
