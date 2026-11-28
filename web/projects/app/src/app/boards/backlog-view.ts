import { CdkDrag, CdkDragDrop, CdkDropList, CdkDropListGroup } from '@angular/cdk/drag-drop';
import {
  Component,
  DestroyRef,
  computed,
  inject,
  input,
  linkedSignal,
  resource,
  signal,
} from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  Backlog,
  BacklogItem,
  BacklogSprint,
  Board,
  ConfirmDialog,
  ConfirmDialogData,
  ErrorState,
  LoadingState,
  LocalDatePipe,
  LocalNumberPipe,
  Realtime,
  Sprint,
  deleteSprint,
  errorTranslationKey,
  getBacklog,
  isApiProblem,
  moveBacklogIssue,
} from 'ui';

import { BACKLOG, ListId, itemsOf, listOfItem, moved, placement, plannable } from './backlog-model';
import { BacklogRow, Shift, Target } from './backlog-row';
import { SprintDialog, SprintDialogData } from './sprint-dialog';

/** How long real-time signals gather before the backlog is read again. */
export const REALTIME_DELAY_MS = 300;

/** A message about the last change: its translation key and parameters. */
interface Message {
  readonly key: string;
  readonly params?: Record<string, string>;
}

/**
 * The backlog of a Scrum board (mockup 17, L09-US01): its open sprints, each
 * with its tickets and total in the board's estimation, the sprints of other
 * boards holding its tickets (read-only here), then the ranked backlog.
 * Planners drag a ticket between the lists or within one (rank), or use its
 * menu with the keyboard: move to a sprint or the backlog, up, down, top,
 * bottom. They create, change and delete future sprints. Changes by others
 * show as they happen.
 */
@Component({
  selector: 'app-backlog',
  imports: [
    BacklogRow,
    CdkDrag,
    CdkDropList,
    CdkDropListGroup,
    ErrorState,
    LoadingState,
    LocalDatePipe,
    LocalNumberPipe,
    MatButtonModule,
    MatIconModule,
    MatMenuModule,
    MatProgressBarModule,
    TranslocoPipe,
  ],
  templateUrl: './backlog-view.html',
  styleUrl: './backlog-view.scss',
})
export class BacklogView {
  readonly board = input.required<Board>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly destroyRef = inject(DestroyRef);

  protected readonly backlogId = BACKLOG;

  protected readonly read = resource({
    params: () => ({ id: this.board().id, version: this.board().version }),
    loader: ({ params }) => this.api.invoke(getBacklog, { id: params.id }),
  });
  /** The backlog as drawn: as read, a ticket moved by the member already in place. */
  protected readonly shown = linkedSignal<Backlog | undefined, Backlog | undefined>({
    source: () => (this.read.hasValue() ? this.read.value() : undefined),
    computation: (read, previous) => read ?? previous?.value,
  });
  protected readonly canPlan = computed(() => this.shown()?.canPlan ?? false);
  /** This board's open sprints: where its tickets may go. */
  protected readonly ownSprints = computed(() =>
    (this.shown()?.sprints ?? []).filter((s) => s.own && s.sprint.state !== 'closed'),
  );
  /** The ticket whose move is under way. */
  protected readonly moving = signal<string | null>(null);
  protected readonly message = signal<Message | null>(null);

  private refreshTimer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    inject(Realtime)
      .events.pipe(takeUntilDestroyed())
      .subscribe((event) => {
        if (event.type !== 'notifications') this.refreshSoon();
      });
    this.destroyRef.onDestroy(() => {
      if (this.refreshTimer) clearTimeout(this.refreshTimer);
    });
  }

  /** Reads the backlog again once the signals of a burst have come. */
  private refreshSoon(): void {
    if (this.refreshTimer) clearTimeout(this.refreshTimer);
    this.refreshTimer = setTimeout(() => {
      this.refreshTimer = null;
      if (!this.moving()) this.read.reload();
    }, REALTIME_DELAY_MS);
  }

  /** Whether tickets may be dragged into and out of `list`. */
  protected movableIn(list: ListId): boolean {
    const backlog = this.shown();
    return !!backlog && this.canPlan() && plannable(backlog, list);
  }

  /** A ticket dropped in a list (or elsewhere in its own). */
  protected dropped(event: CdkDragDrop<ListId, ListId, BacklogItem>): void {
    const from = event.previousContainer.data;
    const to = event.container.data;
    if (from === to && event.previousIndex === event.currentIndex) return;
    void this.move(event.item.data, to, event.currentIndex);
  }

  /** Moves `item` to position `index` of `to` (counted without it): at once, then for real. */
  protected async move(item: BacklogItem, to: ListId, index: number): Promise<void> {
    const backlog = this.shown();
    if (!backlog || this.moving()) return;
    const from = listOfItem(backlog, item.card.id);
    if (!from) return;
    const ids = itemsOf(backlog, to)
      .map((i) => i.card.id)
      .filter((id) => id !== item.card.id);
    const place = placement(ids, index);
    this.message.set(null);
    this.moving.set(item.card.id);
    this.shown.set(moved(backlog, item.card.id, from, to, index));
    try {
      await this.api.invoke(moveBacklogIssue, {
        id: this.board().id,
        body: { issueId: item.card.id, sprintId: to === BACKLOG ? null : to, ...place },
      });
    } catch (error) {
      this.message.set(this.refusal(error, item));
    } finally {
      this.moving.set(null);
      this.read.reload();
    }
  }

  /** Moves `item` within its list by `step` places, or to an end. */
  protected shift(item: BacklogItem, where: Shift): void {
    const backlog = this.shown();
    const list = backlog ? listOfItem(backlog, item.card.id) : null;
    if (!backlog || !list) return;
    const items = itemsOf(backlog, list);
    const index = items.findIndex((i) => i.card.id === item.card.id);
    const last = items.length - 1;
    const target = { up: index - 1, down: index + 1, top: 0, bottom: last }[where];
    if (target < 0 || target > last || target === index) return;
    void this.move(item, list, target);
  }

  /** The lists `item` may be sent to with its menu. */
  protected targetsOf(item: BacklogItem): Target[] {
    const backlog = this.shown();
    const list = backlog ? listOfItem(backlog, item.card.id) : null;
    const targets: Target[] = this.ownSprints()
      .filter((s) => s.sprint.id !== list)
      .map((s) => ({ id: s.sprint.id, sprint: s.sprint }));
    if (list !== BACKLOG) targets.push({ id: BACKLOG, sprint: null });
    return targets;
  }

  /** Sends `item` to the end of `to`. */
  protected sendTo(item: BacklogItem, to: ListId): void {
    const backlog = this.shown();
    if (!backlog) return;
    void this.move(item, to, itemsOf(backlog, to).length);
  }

  /** Why a move was refused, in the member's words. */
  private refusal(error: unknown, item: BacklogItem): Message {
    if (!isApiProblem(error)) return { key: errorTranslationKey('unknown') };
    if (error.code === 'sprint.foreign')
      return { key: 'backlog.refused.foreign', params: { key: item.card.key } };
    if (error.code === 'sprint.subtask')
      return { key: 'backlog.refused.subtask', params: { key: item.card.key } };
    return { key: error.translationKey };
  }

  protected createSprint(): void {
    this.openSprintDialog({ boardId: this.board().id });
  }

  protected editSprint(sprint: Sprint): void {
    this.openSprintDialog({ boardId: this.board().id, sprint });
  }

  private openSprintDialog(data: SprintDialogData): void {
    this.dialog
      .open<SprintDialog, SprintDialogData, Sprint>(SprintDialog, {
        data,
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe((saved) => {
        if (saved) this.read.reload();
      });
  }

  protected deleteSprint(entry: BacklogSprint): void {
    const data: ConfirmDialogData = {
      titleKey: 'backlog.sprint.delete.title',
      messageKey: 'backlog.sprint.delete.message',
      confirmKey: 'backlog.sprint.delete.confirm',
      params: { name: entry.sprint.name, count: String(entry.items.length) },
    };
    this.dialog
      .open(ConfirmDialog, { data })
      .afterClosed()
      .subscribe(async (confirmed?: boolean) => {
        if (!confirmed) return;
        this.message.set(null);
        try {
          await this.api.invoke(deleteSprint, {
            id: entry.sprint.id,
            'If-Match': `"${entry.sprint.version}"`,
          });
        } catch (error) {
          this.message.set({
            key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'),
          });
        } finally {
          this.read.reload();
        }
      });
  }

  protected retry(): void {
    this.read.reload();
  }
}
