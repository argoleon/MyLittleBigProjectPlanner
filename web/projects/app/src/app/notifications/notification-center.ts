import { Injectable, inject, signal } from '@angular/core';
import { Api, countMyUnreadNotifications } from 'ui';

/**
 * How many notifications the member has not read, in all their
 * organizations (L05-US07): read by the bell, updated by the notifications
 * page when it marks some read.
 */
@Injectable({ providedIn: 'root' })
export class NotificationCenter {
  private readonly api = inject(Api);
  private readonly unreadSignal = signal(0);

  readonly unread = this.unreadSignal.asReadonly();

  /** Reads the count again; keeps the last one if the API cannot answer. */
  async refresh(): Promise<void> {
    try {
      this.unreadSignal.set((await this.api.invoke(countMyUnreadNotifications)).total);
    } catch {
      // The bell keeps its count; the page says what went wrong.
    }
  }

  /** Takes the count the API just gave. */
  update(total: number): void {
    this.unreadSignal.set(total);
  }
}
