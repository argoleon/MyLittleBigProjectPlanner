import { Component } from '@angular/core';
import { MatIconModule } from '@angular/material/icon';
import { MatToolbarModule } from '@angular/material/toolbar';
import { RouterOutlet } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { LanguageMenu, ThemeMenu } from 'ui';

/**
 * Shell of the request portal: a top app bar, no drawer (mockup 06). The
 * portal's brand color comes with portal settings (L11, `applyPortalTheme`).
 */
@Component({
  selector: 'portal-root',
  imports: [LanguageMenu, MatIconModule, MatToolbarModule, RouterOutlet, ThemeMenu, TranslocoPipe],
  styleUrl: './app.scss',
  template: `
    <header>
      <mat-toolbar class="top-bar">
        <mat-icon class="brand-icon" aria-hidden="true">support_agent</mat-icon>
        <span class="brand-name">{{ 'app.portalName' | transloco }}</span>
        <span class="spacer"></span>
        <ui-language-menu />
        <ui-theme-menu />
      </mat-toolbar>
    </header>
    <main id="main" class="main">
      <router-outlet />
    </main>
  `,
})
export class App {}
