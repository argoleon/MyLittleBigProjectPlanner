/**
 * Tests of the real-time service (L05-US08) with a fake EventSource: the
 * stream opened once, its signals handed per ticket and for notifications,
 * a resync; the browser's own reconnection left alone, a closed stream
 * opened again after a growing delay with a resync; stopped on sign-out.
 */
import { TestBed } from '@angular/core/testing';

import { EVENT_SOURCE, EVENTS_URL, EventSourceLike, MAX_RETRY_MS, Realtime, RealtimeEvent } from './realtime';

/** A stream the test drives. */
class FakeSource implements EventSourceLike {
  readyState = 0;
  onerror: ((event: Event) => unknown) | null = null;
  onopen: ((event: Event) => unknown) | null = null;
  closed = false;
  private readonly listeners = new Map<string, ((event: MessageEvent) => void)[]>();

  constructor(readonly url: string) {}

  addEventListener(type: string, listener: (event: MessageEvent) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  close(): void {
    this.closed = true;
    this.readyState = 2;
  }

  emit(type: string, data: object): void {
    for (const listener of this.listeners.get(type) ?? []) listener(new MessageEvent(type, { data: JSON.stringify(data) }));
  }

  opened(): void {
    this.readyState = 1;
    this.onopen?.(new Event('open'));
  }

  failed(readyState: number): void {
    this.readyState = readyState;
    this.onerror?.(new Event('error'));
  }
}

describe('Realtime', () => {
  function setUp() {
    const sources: FakeSource[] = [];
    TestBed.configureTestingModule({
      providers: [
        {
          provide: EVENT_SOURCE,
          useValue: (url: string) => {
            const source = new FakeSource(url);
            sources.push(source);
            return source;
          },
        },
      ],
    });
    const realtime = TestBed.inject(Realtime);
    const seen: RealtimeEvent[] = [];
    realtime.events.subscribe((event) => seen.push(event));
    return { realtime, sources, seen };
  }

  afterEach(() => vi.useRealTimers());

  it('opens the stream once and hands its signals', () => {
    const { realtime, sources, seen } = setUp();
    realtime.start();
    realtime.start();
    expect(sources.map((s) => s.url)).toEqual([EVENTS_URL]);
    sources[0].opened();
    expect(realtime.connected()).toBe(true);
    const forTicket: RealtimeEvent[] = [];
    realtime.issue('i142').subscribe((e) => forTicket.push(e));
    const notified: RealtimeEvent[] = [];
    realtime.notifications().subscribe((e) => notified.push(e));

    const signal = { topic: 'issue', kind: 'issue.updated', issueId: 'i142', issueKey: 'PLAT-142', actorId: 'a2' };
    sources[0].emit('issue', signal);
    sources[0].emit('issue', { ...signal, issueId: 'i7', issueKey: 'PLAT-7' });
    sources[0].emit('notifications', {});
    sources[0].emit('resync', {});
    expect(seen.map((e) => e.type)).toEqual(['issue', 'issue', 'notifications', 'resync']);
    expect(forTicket).toEqual([{ type: 'issue', signal }, { type: 'resync' }]);
    expect(notified.map((e) => e.type)).toEqual(['notifications', 'resync']);
  });

  it('lets the browser reconnect by itself, and opens a closed stream again later', () => {
    vi.useFakeTimers();
    const { realtime, sources, seen } = setUp();
    realtime.start();
    sources[0].opened();
    // Still connecting: the browser retries, Last-Event-ID included.
    sources[0].failed(0);
    expect(realtime.connected()).toBe(false);
    expect(sources).toHaveLength(1);
    // Given up (an error answer): opened again after 1 s, then 2 s…
    sources[0].failed(2);
    expect(sources[0].closed).toBe(true);
    vi.advanceTimersByTime(999);
    expect(sources).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(sources).toHaveLength(2);
    expect(seen.map((e) => e.type)).toEqual(['resync']);
    sources[1].failed(2);
    vi.advanceTimersByTime(2_000);
    expect(sources).toHaveLength(3);
    for (let i = 0; i < 10; i++) {
      sources.at(-1)!.failed(2);
      vi.advanceTimersByTime(MAX_RETRY_MS);
    }
    expect(sources).toHaveLength(13);
    // Open again: the delay starts over.
    sources.at(-1)!.opened();
    sources.at(-1)!.failed(2);
    vi.advanceTimersByTime(1_000);
    expect(sources).toHaveLength(14);
  });

  it('stops following on sign-out', () => {
    vi.useFakeTimers();
    const { realtime, sources } = setUp();
    realtime.start();
    sources[0].failed(2);
    realtime.stop();
    vi.advanceTimersByTime(MAX_RETRY_MS);
    expect(sources).toHaveLength(1);
    realtime.start();
    expect(sources).toHaveLength(2);
    realtime.stop();
    expect(sources[1].closed).toBe(true);
  });
});
