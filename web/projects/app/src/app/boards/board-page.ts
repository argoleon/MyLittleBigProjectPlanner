import { CdkDrag, CdkDragDrop, CdkDropList, CdkDropListGroup, moveItemInArray, transferArrayItem } from '@angular/cdk/drag-drop';
import { Component, DestroyRef, computed, inject, input, linkedSignal, resource, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatChipsModule } from '@angular/material/chips';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  Avatar,
  Board,
  BoardCard as Card,
  BoardCards,
  BoardColumnCards,
  BoardLane,
  BoardSwimlanes,
  ErrorState,
  Issue,
  IssueTransition,
  LoadingState,
  Realtime,
  errorTranslationKey,
  getBoard,
  getBoardCards,
  isApiProblem,
  moveBoardCard,
  rankBoardIssue,
} from 'ui';

import { createIssueFlow } from '../issues/issue-flow';
import { TransitionDialog, TransitionDialogData, TransitionOutcome } from '../issues/transition-dialog';
import { placement } from './backlog-model';
import { BacklogView } from './backlog-view';
import { BoardCard } from './board-card';
import { BoardSettingsDialog, BoardSettingsData, BoardSettingsOutcome } from './board-settings-dialog';
import { ChooseTransitionDialog, ChooseTransitionData } from './choose-transition-dialog';

/** Cards a cell shows before "Show N more". */
export const CELL_LIMIT = 20;
/** How long real-time signals gather before the board reads its cards again. */
export const REALTIME_DELAY_MS = 300;

/** Where a card list sits: its lane and column. */
interface Cell {
  readonly lane: number;
  readonly column: string;
}

/** A message about the last move: its translation key and parameters. */
interface Message {
  readonly key: string;
  readonly params?: Record<string, string>;
}

/**
 * A Kanban board (mockup 04, L06-US05): the tickets of its filter in the
 * columns of their status, each column showing its count against its work
 * in progress limit (highlighted beyond it), grouped in swimlanes by
 * assignee or epic, narrowed by quick filters. A card dragged to another
 * column (or moved through its menu, with the keyboard) runs the workflow's
 * transition: one with fields opens its form, several leading there ask
 * which; a refusal puts the card back and tells why. Moves by others show
 * as they happen. Planners reorder the cards of a column (dragged, or
 * moved up and down through the menu): the board's rank (L09-US01).
 *
 * A Scrum board shows its backlog instead (`BacklogView`).
 */
@Component({
  selector: 'app-board-page',
  imports: [
    Avatar,
    BacklogView,
    BoardCard,
    CdkDrag,
    CdkDropList,
    CdkDropListGroup,
    ErrorState,
    LoadingState,
    MatButtonModule,
    MatButtonToggleModule,
    MatChipsModule,
    MatIconModule,
    MatProgressBarModule,
    RouterLink,
    TranslocoPipe,
  ],
  templateUrl: './board-page.html',
  styleUrl: './board-page.scss',
})
export class BoardPage {
  /** Id of the board, from the route. */
  readonly id = input.required<string>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);
  private readonly destroyRef = inject(DestroyRef);

  protected readonly cellLimit = CELL_LIMIT;
  protected readonly board = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getBoard, { id: params }),
  });
  /** The quick filters toggled on, by position. */
  protected readonly quick = signal<readonly number[]>([]);
  /** The swimlanes shown: the board's own until the member switches. */
  protected readonly swimlanes = linkedSignal<Board | undefined, BoardSwimlanes>({
    source: () => (this.board.hasValue() ? this.board.value() : undefined),
    computation: (board, previous) => previous?.value ?? board?.swimlanes ?? 'none',
  });
  protected readonly cards = resource({
    params: () =>
      this.board.hasValue() && this.board.value().kind !== 'scrum'
        ? { id: this.board.value().id, quick: this.quick(), swimlanes: this.swimlanes(), version: this.board.value().version }
        : undefined,
    loader: ({ params }) =>
      this.api.invoke(getBoardCards, {
        id: params.id,
        quick: params.quick.length > 0 ? params.quick.join(',') : undefined,
        swimlanes: params.swimlanes,
      }),
  });
  /**
   * The lanes as drawn: the cards read, a card moved by the member already
   * in its new column until they are read again.
   */
  protected readonly lanes = linkedSignal<BoardCards | undefined, BoardLane[]>({
    source: () => (this.cards.hasValue() ? this.cards.value() : undefined),
    computation: (cards, previous) => (cards ? cards.lanes.map((lane) => ({ ...lane, cells: lane.cells.map((cell) => [...cell]) })) : (previous?.value ?? [])),
  });
  protected readonly columns = computed<BoardColumnCards[]>(() => (this.cards.hasValue() ? this.cards.value().columns : []));
  /** The columns showing all their cards. */
  protected readonly expanded = signal<ReadonlySet<string>>(new Set());
  /** The card whose move is under way. */
  protected readonly moving = signal<string | null>(null);
  protected readonly message = signal<Message | null>(null);
  protected readonly canMove = computed(() => this.board.hasValue() && !this.board.value().node.archived);
  /** The member may reorder the cards of a column. */
  protected readonly canRank = computed(() => this.board.hasValue() && this.board.value().canPlan);
  protected readonly canCreate = computed(
    () => this.board.hasValue() && this.board.value().node.nodeType === 'project' && !this.board.value().node.archived,
  );
  protected readonly gridColumns = computed(() => `repeat(${Math.max(this.columns().length, 1)}, minmax(232px, 1fr))`);

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

  /** Reads the cards again once the signals of a burst have come. */
  private refreshSoon(): void {
    if (this.refreshTimer) clearTimeout(this.refreshTimer);
    this.refreshTimer = setTimeout(() => {
      this.refreshTimer = null;
      if (!this.moving()) this.cards.reload();
    }, REALTIME_DELAY_MS);
  }

  protected toggleQuick(index: number, selected: boolean): void {
    this.quick.update((quick) => (selected ? [...quick, index].sort((a, b) => a - b) : quick.filter((i) => i !== index)));
  }

  protected isQuickOn(index: number): boolean {
    return this.quick().includes(index);
  }

  protected setSwimlanes(value: BoardSwimlanes): void {
    this.swimlanes.set(value);
  }

  protected showAll(column: string): void {
    this.expanded.update((set) => new Set([...set, column]));
  }

  protected shown(cell: readonly Card[], column: string): readonly Card[] {
    return this.expanded().has(column) ? cell : cell.slice(0, CELL_LIMIT);
  }

  protected cellOf(lane: number, column: string): Cell {
    return { lane, column };
  }

  protected laneCount(lane: BoardLane): number {
    return lane.cells.reduce((sum, cell) => sum + cell.length, 0);
  }

  protected wipOf(column: BoardColumnCards): string {
    return column.wip;
  }

  /** A card dropped in a cell: moved there at once, then for real. */
  protected dropped(event: CdkDragDrop<Cell, Cell, Card>): void {
    const from = event.previousContainer.data;
    const to = event.container.data;
    if (this.moving()) return;
    if (from.column === to.column) {
      if (from.lane === to.lane && event.previousIndex !== event.currentIndex) {
        void this.rank(event.item.data, to, event.previousIndex, event.currentIndex);
      }
      return;
    }
    const columns = this.columns().map((c) => c.id);
    const lanes = this.lanes().map((lane) => ({ ...lane, cells: lane.cells.map((cell) => [...cell]) }));
    const source = lanes[from.lane]?.cells[columns.indexOf(from.column)];
    const target = lanes[to.lane]?.cells[columns.indexOf(to.column)];
    if (!source || !target) return;
    const index = source.findIndex((c) => c.id === event.item.data.id);
    if (index < 0) return;
    transferArrayItem(source, target, index, 0);
    this.lanes.set(lanes);
    void this.move(event.item.data, to.column);
  }

  /** Moves `card` of a cell one place up (-1) or down (1). */
  protected step(card: Card, cell: Cell, step: -1 | 1): void {
    const cards = this.cellCards(cell);
    const from = cards.findIndex((c) => c.id === card.id);
    if (from < 0 || from + step < 0 || from + step >= cards.length) return;
    void this.rank(card, cell, from, from + step);
  }

  private cellCards(cell: Cell): readonly Card[] {
    const column = this.columns().findIndex((c) => c.id === cell.column);
    return this.lanes()[cell.lane]?.cells[column] ?? [];
  }

  /** Moves `card` from position `from` to `to` of its cell: at once, then in the board's rank. */
  private async rank(card: Card, cell: Cell, from: number, to: number): Promise<void> {
    const board = this.board.hasValue() ? this.board.value() : undefined;
    if (!board || !this.canRank()) return;
    const column = this.columns().findIndex((c) => c.id === cell.column);
    const lanes = this.lanes().map((lane) => ({ ...lane, cells: lane.cells.map((c) => [...c]) }));
    const cards = lanes[cell.lane]?.cells[column];
    if (!cards) return;
    moveItemInArray(cards, from, to);
    this.lanes.set(lanes);
    const others = cards.filter((c) => c.id !== card.id).map((c) => c.id);
    this.message.set(null);
    this.moving.set(card.id);
    try {
      await this.api.invoke(rankBoardIssue, { id: board.id, body: { issueId: card.id, ...placement(others, to) } });
    } catch (error) {
      this.message.set({ key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.moving.set(null);
      this.cards.reload();
    }
  }

  /**
   * Moves `card` to `column` through the workflow; `transitionId` once the
   * member picked among several. The cards are read again afterwards: a
   * refused card goes back to its column.
   */
  protected async move(card: Card, column: string, transitionId?: string): Promise<void> {
    const board = this.board.hasValue() ? this.board.value() : undefined;
    if (!board) return;
    this.message.set(null);
    this.moving.set(card.id);
    try {
      const result = await this.api.invoke(moveBoardCard, {
        id: board.id,
        'If-Match': `"${card.version}"`,
        body: { issueId: card.id, columnId: column, transitionId },
      });
      if (result.outcome === 'screen') {
        await this.runWithFields(result.issue, result.transitions[0]);
      } else if (result.outcome === 'choose') {
        const picked = await this.choose(card, result.transitions);
        if (picked && picked.fields.length > 0) await this.runWithFields(result.issue, picked);
        else if (picked) {
          this.moving.set(null);
          await this.move({ ...card, version: result.issue.version }, column, picked.id);
          return;
        }
      }
    } catch (error) {
      this.message.set(this.refusal(error, card, column));
    } finally {
      if (this.moving() === card.id) this.moving.set(null);
      this.cards.reload();
    }
  }

  /** Why a move was refused, in the member's words. */
  private refusal(error: unknown, card: Card, column: string): Message {
    const columnName = this.columns().find((c) => c.id === column)?.name ?? '';
    const status = card.status.label ?? card.status.key;
    if (!isApiProblem(error)) return { key: errorTranslationKey('unknown') };
    if (error.code === 'board.no_transition') {
      return { key: 'boards.move.noTransition', params: { key: card.key, status, column: columnName } };
    }
    if (error.status === 403) {
      return { key: 'boards.move.restricted', params: { key: card.key, column: columnName } };
    }
    if (error.status === 412) return { key: 'boards.move.conflict', params: { key: card.key } };
    return { key: error.translationKey };
  }

  /** Opens the form of a transition with fields; resolves once closed. */
  private runWithFields(ticket: Issue, transition: IssueTransition): Promise<void> {
    const data: TransitionDialogData = { ticket, transition };
    return new Promise((resolve) => {
      this.dialog
        .open<TransitionDialog, TransitionDialogData, TransitionOutcome>(TransitionDialog, {
          data,
          width: 'min(560px, 100vw)',
          autoFocus: 'first-tabbable',
        })
        .afterClosed()
        .subscribe(() => resolve());
    });
  }

  /** Asks which of several transitions to run. */
  private choose(card: Card, transitions: IssueTransition[]): Promise<IssueTransition | undefined> {
    const data: ChooseTransitionData = { key: card.key, transitions };
    return new Promise((resolve) => {
      this.dialog
        .open<ChooseTransitionDialog, ChooseTransitionData, IssueTransition>(ChooseTransitionDialog, {
          data,
          width: 'min(440px, 100vw)',
          autoFocus: 'first-tabbable',
        })
        .afterClosed()
        .subscribe((picked) => resolve(picked));
    });
  }

  protected configure(): void {
    if (!this.board.hasValue()) return;
    const data: BoardSettingsData = { board: this.board.value() };
    this.dialog
      .open<BoardSettingsDialog, BoardSettingsData, BoardSettingsOutcome>(BoardSettingsDialog, {
        data,
        width: 'min(840px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe((outcome) => {
        if (outcome === 'deleted') void this.router.navigate(['/boards']);
        else if (outcome) {
          this.quick.set([]);
          this.swimlanes.set(outcome.swimlanes);
          this.board.set(outcome);
        }
      });
  }

  protected async newTicket(): Promise<void> {
    if (!this.board.hasValue()) return;
    await createIssueFlow(this.dialog, this.router, { projectKey: this.board.value().node.key });
  }

  protected retry(): void {
    if (this.board.error()) this.board.reload();
    else this.cards.reload();
  }
}
