import { Component } from '@angular/core';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';

/**
 * Shown on the pages of an archived project: it is read-only, and out of
 * the active lists and of the aggregates until it is restored.
 */
@Component({
  selector: 'app-archived-banner',
  imports: [MatIconModule, TranslocoPipe],
  template: `
    <mat-icon aria-hidden="true">inventory_2</mat-icon>
    <p>
      <strong>{{ 'hierarchy.archiving.banner' | transloco }}</strong>
      {{ 'hierarchy.archiving.bannerHint' | transloco }}
    </p>
  `,
  styleUrl: './archived-banner.scss',
})
export class ArchivedBanner {}
