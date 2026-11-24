import { Component, computed, inject, input, resource } from '@angular/core';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, DurationPipe, ErrorState, LocalDatePipe, addDays, isoWeek, listCapacity, mondayOf, toLocalDate } from 'ui';

/** Weeks shown. */
export const WEEKS = 4;

/** The capacity of a week: its ISO number, Monday and Sunday, and minutes. */
export interface WeekCapacity {
  week: number;
  from: string;
  to: string;
  minutes: number;
}

/** The weeks of `minutes`, one per day from the Monday `from`. */
export function weekTotals(from: string, minutes: readonly number[]): WeekCapacity[] {
  const weeks: WeekCapacity[] = [];
  for (let start = 0; start < minutes.length; start += 7) {
    const monday = addDays(from, start);
    weeks.push({
      week: isoWeek(monday),
      from: monday,
      to: addDays(monday, 6),
      minutes: minutes.slice(start, start + 7).reduce((sum, m) => sum + m, 0),
    });
  }
  return weeks;
}

/**
 * The capacity of a resource over the next 4 weeks, from this week's
 * Monday (mockup 20, L07-US06): hours per week, after public holidays,
 * closures and absences. The load engine (L08) adds the load to it.
 */
@Component({
  selector: 'app-capacity-weeks',
  imports: [DurationPipe, ErrorState, LocalDatePipe, MatProgressBarModule, TranslocoPipe],
  templateUrl: './capacity-weeks.html',
  styleUrl: './capacity-weeks.scss',
})
export class CapacityWeeks {
  /** The resource. */
  readonly resourceId = input.required<string>();

  private readonly api = inject(Api);
  private readonly from = mondayOf(toLocalDate(new Date()));

  protected readonly capacity = resource({
    params: () => ({ resources: this.resourceId(), from: this.from, to: addDays(this.from, WEEKS * 7 - 1) }),
    loader: ({ params }) => this.api.invoke(listCapacity, params),
  });
  protected readonly weeks = computed(() => {
    const item = this.capacity.hasValue() ? this.capacity.value().items[0] : undefined;
    return item ? weekTotals(this.from, item.minutes) : [];
  });

  /** Reads the weeks again: a period, an exception or an absence changed. */
  reload(): void {
    this.capacity.reload();
  }
}
