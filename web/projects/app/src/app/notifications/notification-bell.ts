import { Component, DestroyRef, inject } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { MatBadgeModule } from '@angular/material/badge';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatTooltipModule } from '@angular/material/tooltip';
import { NavigationEnd, Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { filter } from 'rxjs';

import { NotificationCenter } from './notification-center';

/** How often the count is read again while the member stays on a page. */
export const UNREAD_POLL_MS = 60_000;

/**
 * The bell of the top app bar (L05-US07): the notifications not read yet,
 * of all the member's organizations; opens the notifications page. Read
 * again on each navigation and every minute (real time comes with US08).
 */
@Component({
  selector: 'app-notification-bell',
  imports: [MatBadgeModule, MatButtonModule, MatIconModule, MatTooltipModule, RouterLink, TranslocoPipe],
  template: `
    <a
      matIconButton
      routerLink="/notifications"
      class="bell"
      [attr.aria-label]="(center.unread() ? 'notifications.bellUnread' : 'notifications.bell') | transloco: { count: center.unread() }"
      [matTooltip]="'notifications.title' | transloco"
    >
      <mat-icon
        aria-hidden="true"
        [matBadge]="center.unread() > 99 ? '99+' : center.unread()"
        [matBadgeHidden]="center.unread() === 0"
        matBadgeSize="small"
        matBadgeColor="warn"
        matBadgeDescription=""
        >notifications</mat-icon
      >
    </a>
  `,
})
export class NotificationBell {
  protected readonly center = inject(NotificationCenter);

  constructor() {
    void this.center.refresh();
    inject(Router)
      .events.pipe(
        filter((event) => event instanceof NavigationEnd),
        takeUntilDestroyed(),
      )
      .subscribe(() => void this.center.refresh());
    const timer = setInterval(() => void this.center.refresh(), UNREAD_POLL_MS);
    inject(DestroyRef).onDestroy(() => clearInterval(timer));
  }
}
