import { Component, computed, inject, input, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  Api,
  ErrorState,
  LocaleService,
  WorkloadAggregate,
  WorkloadAggregatePeriod,
  WorkloadAggregateScale,
  addDays,
  formatDuration,
  formatLocalDate,
  formatNumber,
  getNodeWorkload,
  getTeamWorkload,
  isoWeek,
  toLocalDate,
} from 'ui';

import { anchorOf } from './heatmap-model';

/** What the card sums: a node of the hierarchy, or a team. */
export type WorkloadOwner = 'node' | 'team';

/** Periods shown at once: six months, or eight weeks. */
export const PERIODS: Readonly<Record<WorkloadAggregateScale, number>> = { month: 6, week: 8 };

/** The days read from `anchor` (a Monday, or the first of a month): first and last, included. */
export function aggregateRange(scale: WorkloadAggregateScale, anchor: string): { from: string; to: string } {
  if (scale === 'week') return { from: anchor, to: addDays(anchor, PERIODS.week * 7 - 1) };
  const [year, month] = anchor.split('-').map(Number);
  return { from: anchor, to: new Date(Date.UTC(year, month - 1 + PERIODS.month, 0)).toISOString().slice(0, 10) };
}

/** `anchor` moved by `steps` screens (back when negative). */
export function moveAnchor(scale: WorkloadAggregateScale, anchor: string, steps: number): string {
  if (scale === 'week') return addDays(anchor, steps * PERIODS.week * 7);
  const [year, month] = anchor.split('-').map(Number);
  return new Date(Date.UTC(year, month - 1 + steps * PERIODS.month, 1)).toISOString().slice(0, 10);
}

/** A period as the card lists it. */
export interface PeriodLine {
  readonly label: string;
  /** The bar's value: the share used, up to 100. */
  readonly value: number;
  readonly text: string;
  readonly over: boolean;
  /** What a screen reader says of the bar. */
  readonly description: string;
}

/** The lines of `data`, in `locale`. */
export function linesOf(data: WorkloadAggregate, locale: string, translate: (key: string, params?: Record<string, string>) => string): PeriodLine[] {
  return data.periods.map((period: WorkloadAggregatePeriod) => {
    const label =
      data.scale === 'week'
        ? translate('workload.heatmap.week', { number: String(isoWeek(period.start)) })
        : formatLocalDate(period.start, locale, { month: 'long', year: 'numeric' });
    const used = period.usedPercent ?? null;
    const percent = used === null ? '–' : formatNumber(used / 100, locale, { style: 'percent' });
    const text = translate('workload.aggregate.line', {
      load: formatDuration(period.loadMinutes, locale),
      capacity: formatDuration(period.capacityMinutes, locale),
      percent,
    });
    const over = used !== null && used > 100;
    return {
      label,
      value: Math.min(used ?? 0, 100),
      text,
      over,
      description: `${label}: ${text}${over ? `, ${translate('workload.aggregate.over')}` : ''}`,
    };
  });
}

/**
 * Load against capacity per month or week (specification §8, L08-US07),
 * for a node of the hierarchy or a team: a bar per period with its hours
 * and share used in text, an overloaded period said in words too.
 */
@Component({
  selector: 'app-workload-card',
  imports: [ErrorState, MatButtonModule, MatButtonToggleModule, MatIconModule, MatProgressBarModule, TranslocoPipe],
  template: `
    <section class="card" aria-labelledby="workload-card-title">
      <div class="card-header">
        <div>
          <h2 class="card-title" id="workload-card-title">{{ 'workload.aggregate.title' | transloco }}</h2>
          <p class="hint">{{ (owner() === 'node' ? 'workload.aggregate.nodeHint' : 'workload.aggregate.teamHint') | transloco }}</p>
        </div>
        <div class="controls">
          <mat-button-toggle-group
            [value]="scale()"
            (change)="setScale($event.value)"
            [attr.aria-label]="'workload.heatmap.scale' | transloco"
            hideSingleSelectionIndicator
          >
            <mat-button-toggle value="month">{{ 'workload.heatmap.scales.month' | transloco }}</mat-button-toggle>
            <mat-button-toggle value="week">{{ 'workload.heatmap.scales.week' | transloco }}</mat-button-toggle>
          </mat-button-toggle-group>
          <button matIconButton type="button" class="previous" [attr.aria-label]="'workload.heatmap.previous' | transloco" (click)="move(-1)">
            <mat-icon aria-hidden="true">chevron_left</mat-icon>
          </button>
          <button matIconButton type="button" class="next" [attr.aria-label]="'workload.heatmap.next' | transloco" (click)="move(1)">
            <mat-icon aria-hidden="true">chevron_right</mat-icon>
          </button>
        </div>
      </div>
      @if (data.error()) {
        <ui-error-state [error]="data.error()" [headingLevel]="3" (retry)="data.reload()" />
      } @else if (!data.hasValue()) {
        <mat-progress-bar mode="indeterminate" [attr.aria-label]="'workload.aggregate.loading' | transloco" />
      } @else {
        @if (data.value().confidentialProjects > 0) {
          <p class="confidential">{{ 'workload.aggregate.confidential' | transloco: { count: data.value().confidentialProjects + '' } }}</p>
        }
        <ul class="periods">
          @for (line of lines(); track line.label) {
            <li class="period" [class.over]="line.over">
              <div class="line">
                <span class="label">{{ line.label }}</span>
                <span class="text">
                  {{ line.text }}
                  @if (line.over) {
                    <span class="over-text">· {{ 'workload.aggregate.over' | transloco }}</span>
                  }
                </span>
              </div>
              <mat-progress-bar mode="determinate" [value]="line.value" [attr.aria-label]="line.description" />
            </li>
          }
        </ul>
      }
    </section>
  `,
  styleUrl: './workload-card.scss',
})
export class WorkloadCard {
  /** What it sums. */
  readonly owner = input.required<WorkloadOwner>();
  /** Its id. */
  readonly ownerId = input.required<string>();

  private readonly api = inject(Api);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);

  protected readonly scale = signal<WorkloadAggregateScale>('month');
  private readonly anchor = signal(anchorOf('month', toLocalDate(new Date())));
  protected readonly data = resource({
    params: () => ({ owner: this.owner(), id: this.ownerId(), scale: this.scale(), ...aggregateRange(this.scale(), this.anchor()) }),
    loader: ({ params }) =>
      params.owner === 'node'
        ? this.api.invoke(getNodeWorkload, { id: params.id, scale: params.scale, from: params.from, to: params.to })
        : this.api.invoke(getTeamWorkload, { id: params.id, scale: params.scale, from: params.from, to: params.to }),
  });
  protected readonly lines = computed(() =>
    this.data.hasValue() ? linesOf(this.data.value(), this.locale.formattingLocale(), (key, params) => this.transloco.translate(key, params)) : [],
  );

  protected setScale(scale: WorkloadAggregateScale): void {
    this.anchor.set(anchorOf(scale, this.anchor()));
    this.scale.set(scale);
  }

  protected move(steps: number): void {
    this.anchor.set(moveAnchor(this.scale(), this.anchor(), steps));
  }
}
