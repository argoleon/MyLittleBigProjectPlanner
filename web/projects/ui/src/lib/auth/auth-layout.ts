import { Component, inject, input } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';

import { LANGUAGES, LANGUAGE_NAMES, LocaleService } from '../i18n/locale.service';

/**
 * Frame of the pages shown before signing in (mockup 07): the product name,
 * one card holding the page, and the language choice below it.
 *
 * ```html
 * <ui-auth-layout>
 *   <h1>…</h1>
 *   <form>…</form>
 * </ui-auth-layout>
 * ```
 */
@Component({
  selector: 'ui-auth-layout',
  imports: [MatButtonModule, MatIconModule, TranslocoPipe],
  styleUrl: './auth-layout.scss',
  template: `
    <div class="page">
      <div class="brand">
        <mat-icon class="brand-icon" aria-hidden="true">{{ brandIcon() }}</mat-icon>
        <span class="brand-name">{{ appNameKey() | transloco }}</span>
      </div>
      <main class="card">
        <ng-content />
      </main>
      <nav class="footer" [attr.aria-label]="'auth.languages' | transloco">
        @for (language of languages; track language) {
          <button
            matButton
            type="button"
            [attr.lang]="language"
            [attr.aria-pressed]="locale.language() === language"
            (click)="locale.use(language)"
          >
            {{ names[language] }}
          </button>
        }
      </nav>
    </div>
  `,
})
export class AuthLayout {
  /** Translation key of the product name. */
  readonly appNameKey = input('app.name');
  /** Material Symbols name of the product icon. */
  readonly brandIcon = input('deployed_code');

  protected readonly locale = inject(LocaleService);
  protected readonly languages = LANGUAGES;
  protected readonly names = LANGUAGE_NAMES;
}
