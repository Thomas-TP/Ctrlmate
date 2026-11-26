import { describe, expect, it } from 'vitest';
import { makeSnapshot } from '../../dev/mock-daemon';
import {
  HISTORY_MAX_POINTS,
  HISTORY_MIN_INTERVAL_MS,
  initialView,
  pushSample,
  reduceConnection,
  reduceSnapshot,
} from '../store/reducers';

describe('history reducer', () => {
  it('throttles samples and skips missing sensors', () => {
    let s = pushSample([], 40, 1000);
    s = pushSample(s, 41, 1000 + HISTORY_MIN_INTERVAL_MS - 1);
    expect(s).toHaveLength(1);
    s = pushSample(s, 42, 1000 + HISTORY_MIN_INTERVAL_MS);
    expect(s.map((p) => p.v)).toEqual([40, 42]);
    expect(pushSample(s, null, 9999)).toBe(s);
  });

  it('keeps a bounded ring buffer', () => {
    let s = pushSample([], 0, 0);
    for (let i = 1; i < HISTORY_MAX_POINTS + 30; i++) s = pushSample(s, i, i * 5000);
    expect(s).toHaveLength(HISTORY_MAX_POINTS);
    expect(s[s.length - 1]?.v).toBe(HISTORY_MAX_POINTS + 29);
  });
});

describe('snapshot reducer', () => {
  it('records history only for sensors that report a value', () => {
    const snap = makeSnapshot('default'); // CPU temperature requires deep hardware: absent
    const v = reduceSnapshot(initialView(0), snap, 5000);
    expect(v.snapshot).toBe(snap);
    expect(v.history.cpuTemp).toHaveLength(0);
    expect(v.history.gpuTemp).toHaveLength(1);
    expect(v.history.gpuTemp[0]?.v).toBe(46);
    expect(v.snapshotAt).toBe(5000);
  });

  it('tracks the CPU temperature once deep hardware provides it', () => {
    const v = reduceSnapshot(initialView(0), makeSnapshot('deep'), 5000);
    expect(v.history.cpuTemp).toHaveLength(1);
  });
});

describe('connection reducer', () => {
  it('stores status and only resets "since" on a status change', () => {
    const v0 = initialView(100);
    const v1 = reduceConnection(v0, { status: 'connected', attempt: 0 }, 200);
    expect(v1.connection.status).toBe('connected');
    expect(v1.connection.since).toBe(200);
    const v2 = reduceConnection(v1, { status: 'connected', attempt: 0 }, 300);
    expect(v2.connection.since).toBe(200);
    const v3 = reduceConnection(
      v2,
      { status: 'unauthorized', reason: 'token_permission_denied', attempt: 1 },
      400,
    );
    expect(v3.connection.reason).toBe('token_permission_denied');
    expect(v3.connection.since).toBe(400);
  });
});
