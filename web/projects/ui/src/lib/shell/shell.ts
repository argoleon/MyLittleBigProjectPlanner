import { BreakpointObserver } from '@angular/cdk/layout';
import { Component, DestroyRef, ElementRef, inject, input, viewChild } from '@angular/core';
import { takeUntilDestroyed, toSignal } from '@angular/core/rxjs-interop';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatListModule } from '@angular/material/list';
import { MatSidenav, MatSidenavModule } from '@angular/material/sidenav';
import { MatToolbarModule } from '@angular/material/toolbar';
import { NavigationEnd, Router, RouterLink, RouterLinkActive } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { filter, map } from 'rxjs';

import { LanguageMenu } from './language-menu';
import { NavItem } from './nav-item';
import { ThemeMenu } from './theme-menu';

/** Below this width (M3 "compact" and "medium"), the drawer is modal. */
export const MODAL_DRAWER_QUERY = '(max-width: 839.98px)';

/**
 * Application shell: navigation drawer, top app bar (language, theme, then
 * what the application adds) and the main area, with a skip link to it.
 *
 * ```html
 * <ui-shell [navItems]="nav">
 *   <ui-tenant-switcher uiShellDrawerHeader … />
 *   <ui-account-menu uiShellTopBar … />
 *   <router-outlet />
 * </ui-shell>
 * ```
 */
@Component({
  selector: 'ui-shell',
  imports: [
    LanguageMenu,
    MatButtonModule,
    MatIconModule,
    MatListModule,
    MatSidenavModule,
    MatToolbarModule,
    RouterLink,
    RouterLinkActive,
    ThemeMenu,
    TranslocoPipe,
  ],
  templateUrl: './shell.html',
  styleUrl: './shell.scss',
})
export class Shell {
  /** Entries of the navigation drawer. */
  readonly navItems = input.required<readonly NavItem[]>();
  /** Translation key of the product name shown in the drawer. */
  readonly appNameKey = input('app.name');
  /** Material Symbols name of the product icon. */
  readonly brandIcon = input('deployed_code');

  protected readonly modal = toSignal(
    inject(BreakpointObserver)
      .observe(MODAL_DRAWER_QUERY)
      .pipe(map((state) => state.matches)),
    { initialValue: false },
  );

  private readonly drawer = viewChild.required(MatSidenav);
  private readonly main = viewChild.required<ElementRef<HTMLElement>>('main');

  constructor() {
    // A modal drawer closes once the user has picked a destination.
    inject(Router)
      .events.pipe(
        filter((event) => event instanceof NavigationEnd),
        takeUntilDestroyed(inject(DestroyRef)),
      )
      .subscribe(() => {
        if (this.modal()) {
          void this.drawer().close();
        }
      });
  }

  protected openDrawer(): void {
    void this.drawer().open();
  }

  /** The skip link moves the focus without changing the URL. */
  protected skipToMain(event: Event): void {
    event.preventDefault();
    this.main().nativeElement.focus();
  }
}
