import { Component, computed, inject, input, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { RouterLink } from '@angular/router';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  Api,
  Assignment,
  AssignmentSegment,
  ConfirmDialog,
  ConfirmDialogData,
  DurationPipe,
  ErrorState,
  LocalDatePipe,
  LocaleService,
  ResourceAssignment,
  deleteAssignment,
  errorTranslationKey,
  getAssignment,
  getIssue,
  isApiProblem,
  listResourceAssignments,
  toLocalDate,
} from 'ui';

import { AssignmentDialog, AssignmentDialogData } from '../issues/assignment-dialog';
import { segmentValueText } from '../issues/segment-text';

/** Assignments read per page. */
export const PAGE_SIZE = 100;

/** The assignments of a project, or the busy time on tickets the member does not see. */
export interface AssignmentGroup {
  /** The project's id; `null` for busy time. */
  readonly id: string | null;
  readonly name: string | null;
  readonly key: string | null;
  readonly items: readonly ResourceAssignment[];
  readonly loadMinutes: number;
}

/** The first day of an assignment. */
function firstDay(assignment: ResourceAssignment): string {
  return assignment.segments.map((s) => s.start).reduce((a, b) => (a < b ? a : b), '9999-12-31');
}

/**
 * `assignments` by project, by project name, each by its first day; busy
 * time (tickets the member does not see) last.
 */
export function groupByProject(assignments: readonly ResourceAssignment[]): AssignmentGroup[] {
  const groups = new Map<string, { id: string | null; name: string | null; key: string | null; items: ResourceAssignment[] }>();
  for (const assignment of assignments) {
    const project = assignment.busy ? null : (assignment.issue?.project ?? null);
    const id = project?.id ?? '';
    const group = groups.get(id) ?? { id: project?.id ?? null, name: project?.name ?? null, key: project?.key ?? null, items: [] };
    group.items.push(assignment);
    groups.set(id, group);
  }
  return [...groups.values()]
    .map((group) => ({
      ...group,
      items: [...group.items].sort((a, b) => firstDay(a).localeCompare(firstDay(b))),
      loadMinutes: group.items.reduce((sum, a) => sum + a.loadMinutes, 0),
    }))
    .sort((a, b) => (a.id === null ? 1 : b.id === null ? -1 : (a.name ?? '').localeCompare(b.name ?? '')));
}

/**
 * The "Assignments" tab of a resource (mockup 20, L08-US05): its
 * assignments grouped by project, each with its ticket, periods, values
 * and load, ongoing and upcoming ones first, past ones on demand. Time on
 * tickets the member does not see shows as busy, without ticket nor
 * project. Those who assign resources on a project change an assignment
 * through the dialog of mockup 21, or remove it after confirmation.
 */
@Component({
  selector: 'app-resource-assignments',
  imports: [
    DurationPipe,
    ErrorState,
    LocalDatePipe,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatSlideToggleModule,
    RouterLink,
    TranslocoPipe,
  ],
  templateUrl: './resource-assignments.html',
  styleUrl: './resource-assignments.scss',
})
export class ResourceAssignments {
  /** The resource. */
  readonly resourceId = input.required<string>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);

  /** Past assignments are listed too. */
  protected readonly showPast = signal(false);
  private readonly today = toLocalDate(new Date());

  protected readonly first = resource({
    params: () => ({ id: this.resourceId(), from: this.showPast() ? undefined : this.today }),
    loader: ({ params }) =>
      this.api.invoke(listResourceAssignments, { id: params.id, from: params.from, expand: 'issue', limit: PAGE_SIZE }),
  });
  /** The pages read after the first one. */
  private readonly more = signal<readonly ResourceAssignment[]>([]);
  private readonly cursor = signal<string | null | undefined>(undefined);
  protected readonly loadingMore = signal(false);
  protected readonly notice = signal<{ ok: boolean; messageKey: string; params?: Record<string, string> } | null>(null);

  protected readonly items = computed<readonly ResourceAssignment[]>(() => [
    ...(this.first.hasValue() ? this.first.value().items : []),
    ...this.more(),
  ]);
  protected readonly groups = computed(() => groupByProject(this.items()));
  protected readonly next = computed(() => {
    const read = this.cursor();
    if (read !== undefined) return read;
    return this.first.hasValue() ? this.first.value().nextCursor : null;
  });

  /** What a segment's value reads. */
  protected valueText(segment: AssignmentSegment): string {
    return segmentValueText(segment, this.locale.formattingLocale(), (key, params) => this.transloco.translate(key, params));
  }

  /** Lists past assignments too, or not: the list reads its first page again. */
  protected togglePast(show: boolean): void {
    this.more.set([]);
    this.cursor.set(undefined);
    this.showPast.set(show);
  }

  /** Reads the list again from its first page. */
  private reset(): void {
    this.more.set([]);
    this.cursor.set(undefined);
    this.first.reload();
  }

  protected async loadMore(): Promise<void> {
    const cursor = this.next();
    if (!cursor) return;
    this.loadingMore.set(true);
    try {
      const page = await this.api.invoke(listResourceAssignments, {
        id: this.resourceId(),
        from: this.showPast() ? undefined : this.today,
        expand: 'issue',
        limit: PAGE_SIZE,
        cursor,
      });
      this.more.update((items) => [...items, ...page.items]);
      this.cursor.set(page.nextCursor);
    } catch (error) {
      this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.loadingMore.set(false);
    }
  }

  /** Opens the assignment dialog on `item`, with its ticket. */
  protected async edit(item: ResourceAssignment): Promise<void> {
    if (!item.issueId) return;
    this.notice.set(null);
    let assignment: Assignment;
    let data: AssignmentDialogData;
    try {
      assignment = await this.api.invoke(getAssignment, { id: item.id });
      data = { issue: await this.api.invoke(getIssue, { idOrKey: item.issueId }), assignment };
    } catch (error) {
      this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
      return;
    }
    this.dialog
      .open<AssignmentDialog, AssignmentDialogData, Assignment>(AssignmentDialog, {
        data,
        width: 'min(880px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: `#resource-assignment-edit-${item.id}`,
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({ ok: true, messageKey: 'resources.assignments.saved', params: { key: data.issue.key } });
        this.reset();
      });
  }

  protected remove(item: ResourceAssignment): void {
    const key = item.issue?.key ?? '';
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, {
        data: {
          titleKey: 'resources.assignments.deleteTitle',
          messageKey: 'resources.assignments.deleteMessage',
          confirmKey: 'resources.assignments.deleteConfirm',
          params: { key },
        },
        width: 'min(480px, 100vw)',
        restoreFocus: '#resource-assignments-title',
      })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteAssignment, { id: item.id, 'If-Match': `"${item.version}"` });
          this.notice.set({ ok: true, messageKey: 'resources.assignments.deleted', params: { key } });
          this.reset();
        } catch (error) {
          this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }
}
