import { Component, input } from '@angular/core';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { BulkAction } from 'ui';

import { reasonKey } from './bulk-flow';

/**
 * The report of a bulk action (L06-US08): how many tickets changed, were
 * already so, or were refused, then what became of each, with the reason
 * or its new key.
 */
@Component({
  selector: 'app-bulk-report',
  imports: [RouterLink, TranslocoPipe],
  template: `
    @let r = report();
    <p class="counts" role="status">
      {{ 'bulk.report.counts' | transloco: { changed: r.counts.changed + r.counts.withParent, unchanged: r.counts.unchanged, refused: r.counts.refused } }}
      @if (r.counts.waiting > 0) {
        · {{ 'bulk.report.waiting' | transloco: { count: r.counts.waiting } }}
      }
    </p>
    <table class="items">
      <caption class="cdk-visually-hidden">{{ 'bulk.report.caption' | transloco }}</caption>
      <thead>
        <tr>
          <th scope="col">{{ 'bulk.report.ticket' | transloco }}</th>
          <th scope="col">{{ 'bulk.report.outcome' | transloco }}</th>
        </tr>
      </thead>
      <tbody>
        @for (item of r.items; track item.issueId) {
          <tr [attr.data-outcome]="item.outcome ?? 'waiting'">
            <td class="key">
              <a [routerLink]="['/issues', item.newKey ?? item.key]">{{ item.key }}</a>
            </td>
            <td class="outcome">
              @switch (item.outcome) {
                @case ('changed') {
                  @if (item.newKey) {
                    {{ 'bulk.report.movedTo' | transloco: { key: item.newKey } }}
                  } @else {
                    {{ 'bulk.outcomes.changed' | transloco }}
                  }
                }
                @case ('refused') {
                  <span class="refused"
                    >{{ reasonKey(item.reason) | transloco }}@if (item.errors.length > 0) {
                      ({{ fields(item.errors) }})
                    }</span
                  >
                }
                @case ('unchanged') {
                  {{ 'bulk.outcomes.unchanged' | transloco }}
                }
                @case ('with_parent') {
                  {{ 'bulk.outcomes.with_parent' | transloco }}
                }
                @default {
                  {{ 'bulk.outcomes.waiting' | transloco }}
                }
              }
            </td>
          </tr>
        }
      </tbody>
    </table>
  `,
  styleUrl: './bulk-report.scss',
})
export class BulkReport {
  readonly report = input.required<BulkAction>();

  protected readonly reasonKey = reasonKey;

  protected fields(errors: readonly { field: string; code: string }[]): string {
    return errors.map((e) => e.field).join(', ');
  }
}
