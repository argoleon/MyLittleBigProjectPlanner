import { Component, inject } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { TranslocoPipe } from '@jsverse/transloco';

import { THEME_MODES, ThemeMode, ThemeService } from '../theming/theme.service';

const ICONS: Readonly<Record<ThemeMode, string>> = {
  system: 'brightness_auto',
  light: 'light_mode',
  dark: 'dark_mode',
};

/** Icon button choosing between the system, light and dark modes. */
@Component({
  selector: 'ui-theme-menu',
  imports: [MatButtonModule, MatIconModule, MatMenuModule, TranslocoPipe],
  template: `
    <button
      matIconButton
      type="button"
      [matMenuTriggerFor]="menu"
      [attr.aria-label]="'shell.theme.label' | transloco: { mode: ('shell.theme.' + theme.mode() | transloco) }"
    >
      <mat-icon aria-hidden="true">{{ icons[theme.mode()] }}</mat-icon>
    </button>
    <mat-menu #menu="matMenu">
      @for (mode of modes; track mode) {
        <button
          mat-menu-item
          type="button"
          role="menuitemradio"
          [attr.aria-checked]="theme.mode() === mode"
          (click)="theme.use(mode)"
        >
          <mat-icon aria-hidden="true">{{ icons[mode] }}</mat-icon>
          <span>{{ 'shell.theme.' + mode | transloco }}</span>
        </button>
      }
    </mat-menu>
  `,
})
export class ThemeMenu {
  protected readonly theme = inject(ThemeService);
  protected readonly modes = THEME_MODES;
  protected readonly icons = ICONS;
}
