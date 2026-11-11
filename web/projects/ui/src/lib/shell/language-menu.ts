import { Component, computed, inject } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { TranslocoPipe } from '@jsverse/transloco';

import { LANGUAGES, LANGUAGE_NAMES, LocaleService } from '../i18n/locale.service';

/** Icon button opening the list of interface languages. */
@Component({
  selector: 'ui-language-menu',
  imports: [MatButtonModule, MatIconModule, MatMenuModule, TranslocoPipe],
  template: `
    <button
      matIconButton
      type="button"
      [matMenuTriggerFor]="menu"
      [attr.aria-label]="'shell.language' | transloco: { language: currentName() }"
    >
      <mat-icon aria-hidden="true">translate</mat-icon>
    </button>
    <mat-menu #menu="matMenu">
      @for (language of languages; track language) {
        <button
          mat-menu-item
          type="button"
          role="menuitemradio"
          [attr.aria-checked]="locale.language() === language"
          [attr.lang]="language"
          (click)="locale.pick(language)"
        >
          <mat-icon aria-hidden="true">{{ locale.language() === language ? 'check' : '' }}</mat-icon>
          <span>{{ names[language] }}</span>
        </button>
      }
    </mat-menu>
  `,
})
export class LanguageMenu {
  protected readonly locale = inject(LocaleService);
  protected readonly languages = LANGUAGES;
  protected readonly names = LANGUAGE_NAMES;
  protected readonly currentName = computed(() => LANGUAGE_NAMES[this.locale.language()]);
}
