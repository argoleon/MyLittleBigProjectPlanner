import { Component, computed, inject, input, output, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  CapacityPattern,
  ConfirmDialog,
  ConfirmDialogData,
  DurationPipe,
  ErrorState,
  LocalDatePipe,
  LocaleService,
  deleteCapacityPattern,
  errorTranslationKey,
  isApiProblem,
  listCapacityPatterns,
  toLocalDate,
  weekdayNames,
} from 'ui';

import { PeriodDialog, PeriodDialogData } from './period-dialog';

/** Outcome of the last action. */
interface Notice {
  readonly ok: boolean;
  readonly messageKey: string;
}

/** A row of the table: a period, or the organization's default when there is none. */
export type CapacityRow = CapacityPattern & { readonly isDefault?: boolean };

/** The days of the week, Monday first, as table columns. */
const DAYS = ['d0', 'd1', 'd2', 'd3', 'd4', 'd5', 'd6'];

/**
 * The weekly capacity of a resource (mockup 20, L07-US03): its periods by
 * effective date, each with its hours per weekday and weekly total. Before
 * the first period, or without any, the organization's default applies.
 * Tenant admins and resource managers add, change and delete periods.
 */
@Component({
  selector: 'app-capacity-card',
  imports: [DurationPipe, ErrorState, LocalDatePipe, MatButtonModule, MatIconModule, MatProgressBarModule, MatTableModule, TranslocoPipe],
  templateUrl: './capacity-card.html',
  styleUrl: './capacity-card.scss',
})
export class CapacityCard {
  /** The resource. */
  readonly resourceId = input.required<string>();
  /** A period was added, changed or deleted. */
  readonly changed = output<void>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);

  protected readonly list = resource({
    params: () => this.resourceId(),
    loader: ({ params }) => this.api.invoke(listCapacityPatterns, { id: params }),
  });
  protected readonly periods = computed<readonly CapacityPattern[]>(() => (this.list.hasValue() ? this.list.value().items : []));
  protected readonly defaultMinutes = computed<readonly number[]>(() => (this.list.hasValue() ? this.list.value().defaultMinutes : []));
  protected readonly canManage = computed(() => this.list.hasValue() && this.list.value().canManage);
  /** The periods, or the organization's default alone. */
  protected readonly rows = computed<readonly CapacityRow[]>(() => {
    if (this.periods().length > 0) return this.periods();
    const minutes = this.defaultMinutes();
    return [{ id: 'default', effectiveFrom: '', minutes: [...minutes], weeklyMinutes: this.total(minutes), version: 0, isDefault: true }];
  });
  protected readonly days = DAYS;
  protected readonly dayNames = computed(() => weekdayNames(this.locale.formattingLocale(), 'short'));
  protected readonly dayLongNames = computed(() => weekdayNames(this.locale.formattingLocale()));
  protected readonly columns = computed(() => ['from', ...DAYS, 'total', ...(this.canManage() ? ['actions'] : [])]);
  protected readonly notice = signal<Notice | null>(null);
  /** Today, in the user's calendar: a period starting later is planned. */
  private readonly today = toLocalDate(new Date());

  protected readonly trackPeriod = (_: number, row: CapacityRow) => row.id;

  protected planned(period: CapacityRow): boolean {
    return !period.isDefault && period.effectiveFrom > this.today;
  }

  protected total(minutes: readonly number[]): number {
    return minutes.reduce((sum, value) => sum + value, 0);
  }

  protected actionId(period: CapacityPattern, action: 'edit' | 'delete'): string {
    return `period-${action}-${period.id}`;
  }

  protected create(): void {
    const latest = this.periods().at(-1);
    this.open({ resourceId: this.resourceId(), startingWeek: latest?.minutes ?? this.defaultMinutes() }, undefined);
  }

  protected edit(period: CapacityPattern): void {
    this.open({ resourceId: this.resourceId(), period, startingWeek: period.minutes }, `#${this.actionId(period, 'edit')}`);
  }

  protected remove(period: CapacityPattern): void {
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, {
        data: {
          titleKey: 'capacity.deleteTitle',
          messageKey: 'capacity.deleteMessage',
          confirmKey: 'capacity.deleteConfirm',
        },
        width: 'min(480px, 100vw)',
        restoreFocus: `#${this.actionId(period, 'delete')}`,
      })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteCapacityPattern, { id: this.resourceId(), patternId: period.id, 'If-Match': `"${period.version}"` });
          this.notice.set({ ok: true, messageKey: 'capacity.deleted' });
          this.list.reload();
          this.changed.emit();
        } catch (error) {
          this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }

  private open(data: PeriodDialogData, restoreFocus: string | undefined): void {
    this.dialog
      .open<PeriodDialog, PeriodDialogData, CapacityPattern>(PeriodDialog, {
        data,
        width: 'min(640px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: restoreFocus ?? true,
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({ ok: true, messageKey: data.period ? 'capacity.saved' : 'capacity.created' });
        this.list.reload();
        this.changed.emit();
      });
  }
}
