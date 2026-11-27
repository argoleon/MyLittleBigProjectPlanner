import { Component, computed, inject, resource, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatChipsModule } from '@angular/material/chips';
import { provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AvailableResource,
  DurationPipe,
  ErrorState,
  LocalDatePipe,
  addDays,
  findAvailability,
  fromLocalDate,
  listTeams,
  mondayOf,
  toLocalDate,
} from 'ui';

/** What a search asks for: free days, or free hours. */
export type Need = 'days' | 'hours';

/** The longest period searched, in days (as the API). */
export const MAX_DAYS = 92;
/** The most free hours a search may ask for (as the API). */
export const MAX_HOURS = 2000;

/** A search. */
export interface AvailabilityCriteria {
  readonly from: string;
  readonly to: string;
  readonly teamId: string | null;
  readonly need: Need;
  /** At least this many free days or hours; `null`: everyone. */
  readonly amount: number | null;
  readonly includeProvisional: boolean;
}

/** Next week, Monday to Sunday, after `today`. */
export function nextWeek(today: string): { from: string; to: string } {
  const from = addDays(mondayOf(today), 7);
  return { from, to: addDays(from, 6) };
}

/** The days from `from` to `to`, both included. */
export function daysBetween(from: string, to: string): number {
  return Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / 86_400_000) + 1;
}

/** Why a period cannot be searched, as a translation key; `null` when it can. */
export function periodError(from: string, to: string): string | null {
  if (to < from) return 'workload.availability.errors.beforeStart';
  if (daysBetween(from, to) > MAX_DAYS) return 'workload.availability.errors.tooLong';
  return null;
}

/** The heatmap on a resource found: its days over the period, its team, its details. */
export function heatmapLink(
  item: AvailableResource,
  criteria: Pick<AvailabilityCriteria, 'from' | 'teamId' | 'includeProvisional'>,
): Record<string, string> {
  return {
    scale: 'day',
    date: criteria.from,
    resourceId: item.resource.id,
    ...(criteria.teamId ? { teamId: criteria.teamId } : {}),
    ...(criteria.includeProvisional ? {} : { provisional: 'false' }),
  };
}

/**
 * The availability search of the Workload screen (specification §8,
 * L08-US10): who has a number of free days (working days entirely free) or
 * free hours over a period, optionally in a team, most free time first.
 * Absences and public holidays are deducted; load on tickets the member
 * does not see counts without being named. Each resource found opens in
 * the heatmap, on its days.
 */
@Component({
  selector: 'app-availability-page',
  imports: [
    DurationPipe,
    ErrorState,
    LocalDatePipe,
    MatButtonModule,
    MatButtonToggleModule,
    MatChipsModule,
    MatDatepickerModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatProgressBarModule,
    MatSelectModule,
    ReactiveFormsModule,
    RouterLink,
    TranslocoPipe,
  ],
  providers: [provideNativeDateAdapter()],
  templateUrl: './availability-page.html',
  styleUrl: './availability-page.scss',
})
export class AvailabilityPage {
  private readonly api = inject(Api);

  private readonly week = nextWeek(toLocalDate(new Date()));
  protected readonly form = new FormGroup({
    start: new FormControl<Date | null>(fromLocalDate(this.week.from), Validators.required),
    end: new FormControl<Date | null>(fromLocalDate(this.week.to), Validators.required),
    teamId: new FormControl<string | null>(null),
    need: new FormControl<Need>('days', { nonNullable: true }),
    amount: new FormControl<number | null>(3, [Validators.min(1), Validators.max(MAX_DAYS)]),
  });
  protected readonly includeProvisional = signal(true);
  /** Why the period cannot be searched. */
  protected readonly periodProblem = signal<string | null>(null);
  protected readonly need = signal<Need>('days');
  protected readonly maxAmount = computed(() => (this.need() === 'days' ? MAX_DAYS : MAX_HOURS));

  protected readonly teams = resource({ loader: () => this.api.invoke(listTeams, {}) });
  /** The search shown: next week's 3 free days at first. */
  protected readonly criteria = signal<AvailabilityCriteria>({
    ...this.week,
    teamId: null,
    need: 'days',
    amount: 3,
    includeProvisional: true,
  });
  protected readonly results = resource({
    params: () => this.criteria(),
    loader: ({ params }) =>
      this.api.invoke(findAvailability, {
        from: params.from,
        to: params.to,
        teamId: params.teamId ?? undefined,
        minDays: params.need === 'days' ? (params.amount ?? undefined) : undefined,
        minHours: params.need === 'hours' ? (params.amount ?? undefined) : undefined,
        includeProvisional: params.includeProvisional,
      }),
  });
  protected readonly items = computed<readonly AvailableResource[]>(() => (this.results.hasValue() ? this.results.value().items : []));

  /** Free days or free hours: the bounds of the amount follow. */
  protected setNeed(need: Need): void {
    this.need.set(need);
    const amount = this.form.controls.amount;
    amount.setValidators([Validators.min(1), Validators.max(need === 'days' ? MAX_DAYS : MAX_HOURS)]);
    amount.updateValueAndValidity();
  }

  protected search(): void {
    const { start, end, teamId, need, amount } = this.form.getRawValue();
    this.form.markAllAsTouched();
    if (!start || !end || this.form.invalid) return;
    const from = toLocalDate(start);
    const to = toLocalDate(end);
    const problem = periodError(from, to);
    this.periodProblem.set(problem);
    if (problem) return;
    this.criteria.set({
      from,
      to,
      teamId,
      need,
      amount: amount === null || Number.isNaN(amount) ? null : Math.floor(amount),
      includeProvisional: this.includeProvisional(),
    });
  }

  /** The heatmap on `item`, over the period and options of the results shown. */
  protected heatmapLink(item: AvailableResource): Record<string, string> {
    const shown = this.results.value();
    const criteria = this.criteria();
    return heatmapLink(item, {
      from: shown?.from ?? criteria.from,
      teamId: criteria.teamId,
      includeProvisional: shown?.includeProvisional ?? criteria.includeProvisional,
    });
  }
}
