import { Injectable, InjectionToken, OnDestroy, inject, signal } from '@angular/core';
import { Observable, Subject, filter } from 'rxjs';

import { RealtimeSignal } from './generated/models/realtime-signal';

/** What the stream says: a ticket changed, new notifications, or "read everything again". */
export type RealtimeEvent =
  | { readonly type: 'issue'; readonly signal: RealtimeSignal }
  | { readonly type: 'notifications' }
  | { readonly type: 'resync' };

/** The part of `EventSource` the service uses (tests provide a fake one). */
export interface EventSourceLike {
  readonly readyState: number;
  onerror: ((event: Event) => unknown) | null;
  onopen: ((event: Event) => unknown) | null;
  addEventListener(type: string, listener: (event: MessageEvent) => void): void;
  close(): void;
}

/** A stream that never says anything: where `EventSource` does not exist. */
function inert(): EventSourceLike {
  return { readyState: 2, onerror: null, onopen: null, addEventListener: () => undefined, close: () => undefined };
}

/** Opens the stream at `url`. */
export const EVENT_SOURCE = new InjectionToken<(url: string) => EventSourceLike>('EVENT_SOURCE', {
  providedIn: 'root',
  factory: () => (url) => (typeof EventSource === 'undefined' ? inert() : new EventSource(url, { withCredentials: true })),
});

/** The stream of real-time signals. */
export const EVENTS_URL = '/api/v1/events';
/** Longest wait before opening the stream again after the browser gave up. */
export const MAX_RETRY_MS = 30_000;
const CLOSED = 2;

/**
 * Real-time signals of the member's tenant (L05-US08), over Server-Sent
 * Events: which ticket changed, new notifications. Signals carry no data:
 * views read it again through the API. The browser reconnects by itself
 * after a cut and resumes where it was (`Last-Event-ID`); when it gives up
 * (the server answered an error), the service opens the stream again after
 * a growing delay. A `resync` tells the views to read everything again.
 */
@Injectable({ providedIn: 'root' })
export class Realtime implements OnDestroy {
  private readonly open = inject(EVENT_SOURCE);
  private readonly eventsSubject = new Subject<RealtimeEvent>();
  private source: EventSourceLike | null = null;
  private retry: ReturnType<typeof setTimeout> | null = null;
  private delay = 1_000;
  private readonly connectedSignal = signal(false);

  /** Every event, as it comes. */
  readonly events: Observable<RealtimeEvent> = this.eventsSubject.asObservable();
  /** Whether the stream is open now. */
  readonly connected = this.connectedSignal.asReadonly();

  /** The changes of the ticket `issueId`, and the resyncs. */
  issue(issueId: string): Observable<RealtimeEvent> {
    return this.events.pipe(filter((e) => e.type === 'resync' || (e.type === 'issue' && e.signal.issueId === issueId)));
  }

  /** New notifications, and the resyncs. */
  notifications(): Observable<RealtimeEvent> {
    return this.events.pipe(filter((e) => e.type === 'notifications' || e.type === 'resync'));
  }

  /** Follows the stream; nothing if it does already. */
  start(): void {
    if (this.source || this.retry) return;
    const source = this.open(EVENTS_URL);
    this.source = source;
    source.onopen = () => {
      this.connectedSignal.set(true);
      this.delay = 1_000;
    };
    source.onerror = () => {
      this.connectedSignal.set(false);
      // Still CONNECTING: the browser retries by itself, Last-Event-ID included.
      if (source.readyState === CLOSED) this.later();
    };
    source.addEventListener('issue', (event) => {
      try {
        this.eventsSubject.next({ type: 'issue', signal: JSON.parse(event.data) as RealtimeSignal });
      } catch {
        // Not a signal: ignored.
      }
    });
    source.addEventListener('notifications', () => this.eventsSubject.next({ type: 'notifications' }));
    source.addEventListener('resync', () => this.eventsSubject.next({ type: 'resync' }));
  }

  /** Stops following (signed out). */
  stop(): void {
    if (this.retry) clearTimeout(this.retry);
    this.retry = null;
    this.source?.close();
    this.source = null;
    this.connectedSignal.set(false);
  }

  /** Opens the stream again after a growing delay; what was missed meanwhile is read again. */
  private later(): void {
    this.source?.close();
    this.source = null;
    const delay = this.delay;
    this.delay = Math.min(this.delay * 2, MAX_RETRY_MS);
    this.retry = setTimeout(() => {
      this.retry = null;
      this.start();
      this.eventsSubject.next({ type: 'resync' });
    }, delay);
  }

  ngOnDestroy(): void {
    this.stop();
  }
}
