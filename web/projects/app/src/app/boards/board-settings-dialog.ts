import { Component, computed, inject, resource, signal } from '@angular/core';
import { FormArray, FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialog, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  Api,
  Board,
  BoardEstimation,
  BoardSwimlanes,
  ConfirmDialog,
  ConfirmDialogData,
  Node,
  NodeItem,
  createBoard,
  deleteBoard,
  errorTranslationKey,
  fieldErrorTranslationKey,
  getNode,
  isApiProblem,
  listBoardStatuses,
  listNodes,
  updateBoard,
} from 'ui';

import { queryErrorMessage } from '../search/query-error';

/** A board to change, or a new one (on `nodeId` when given). */
export interface BoardSettingsData {
  readonly board?: Board;
  readonly nodeId?: string;
}

/** The board saved, `deleted`, or nothing when cancelled. */
export type BoardSettingsOutcome = Board | 'deleted';

/** Most columns, as the API allows. */
export const MAX_COLUMNS = 12;
/** Most quick filters, as the API allows. */
export const MAX_QUICK_FILTERS = 10;

type ColumnForm = FormGroup<{
  name: FormControl<string>;
  statuses: FormControl<string[]>;
  wipLimit: FormControl<number | null>;
}>;

type QuickForm = FormGroup<{
  label: FormControl<string>;
  query: FormControl<string>;
}>;

function isNode(item: NodeItem): item is Node {
  const node = item as Partial<Node>;
  return typeof node.name === 'string' && node.archived === false;
}

/**
 * Creates or changes a Kanban board (L06-US05). A new board takes its node,
 * name and filter: its columns (one per status category) and quick filters
 * come by default. Changing it also sets its columns (statuses, work in
 * progress limit, order) and quick filters; it may be deleted there.
 */
@Component({
  selector: 'app-board-settings-dialog',
  imports: [MatButtonModule, MatDialogModule, MatFormFieldModule, MatIconModule, MatInputModule, MatSelectModule, ReactiveFormsModule, TranslocoPipe],
  templateUrl: './board-settings-dialog.html',
  styleUrl: './board-settings-dialog.scss',
})
export class BoardSettingsDialog {
  protected readonly data = inject<BoardSettingsData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly dialogRef = inject<MatDialogRef<BoardSettingsDialog, BoardSettingsOutcome>>(MatDialogRef);
  private readonly transloco = inject(TranslocoService);

  protected readonly editing = !!this.data.board;
  protected readonly maxColumns = MAX_COLUMNS;
  protected readonly maxQuick = MAX_QUICK_FILTERS;
  protected readonly estimations: readonly BoardEstimation[] = ['points', 'hours'];
  protected readonly swimlaneModes: readonly BoardSwimlanes[] = ['none', 'assignee', 'epic'];

  protected readonly form = new FormGroup({
    nodeId: new FormControl(this.data.board?.node.id ?? this.data.nodeId ?? '', { nonNullable: true, validators: [Validators.required] }),
    name: new FormControl(this.data.board?.name ?? '', { nonNullable: true, validators: [Validators.required, Validators.maxLength(100)] }),
    filter: new FormControl(this.data.board?.filter ?? '', { nonNullable: true }),
    estimation: new FormControl<BoardEstimation>(this.data.board?.estimation ?? 'points', { nonNullable: true }),
    swimlanes: new FormControl<BoardSwimlanes>(this.data.board?.swimlanes ?? 'none', { nonNullable: true }),
    columns: new FormArray<ColumnForm>((this.data.board?.columns ?? []).map((c) => this.columnForm(c.name, c.statuses, c.wipLimit ?? null))),
    quickFilters: new FormArray<QuickForm>((this.data.board?.quickFilters ?? []).map((q) => this.quickForm(q.label, q.query))),
  });

  /** The nodes a new board may go on. */
  protected readonly nodes = resource({
    params: () => (this.editing ? undefined : true),
    loader: () => this.api.invoke(listNodes, { limit: 200 }),
  });
  /** The node asked for, which may not be among the first nodes listed. */
  protected readonly preselected = resource({
    params: () => (this.editing ? undefined : this.data.nodeId),
    loader: ({ params }) => this.api.invoke(getNode, { id: params }),
  });
  protected readonly nodeChoices = computed(() => {
    const listed = this.nodes.hasValue() ? this.nodes.value().items.filter(isNode) : [];
    const asked = this.preselected.hasValue() ? this.preselected.value() : undefined;
    return asked && !asked.archived && !listed.some((n) => n.id === asked.id) ? [asked, ...listed] : listed;
  });
  /** The statuses columns may show: those of the projects of the board's node. */
  protected readonly statuses = resource({
    params: () => (this.editing ? this.data.board?.node.id : undefined),
    loader: ({ params }) => this.api.invoke(listBoardStatuses, { nodeId: params }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  /** Why the filter was refused, as the screen says it. */
  protected readonly filterError = signal<{ key: string; params: Record<string, string | number> } | null>(null);

  protected get columns(): FormArray<ColumnForm> {
    return this.form.controls.columns;
  }

  protected get quickFilters(): FormArray<QuickForm> {
    return this.form.controls.quickFilters;
  }

  private columnForm(name: string, statuses: string[], wipLimit: number | null): ColumnForm {
    return new FormGroup({
      name: new FormControl(name, { nonNullable: true, validators: [Validators.required, Validators.maxLength(40)] }),
      statuses: new FormControl(statuses, { nonNullable: true }),
      wipLimit: new FormControl<number | null>(wipLimit, { validators: [Validators.min(1), Validators.max(999)] }),
    });
  }

  private quickForm(label: string, query: string): QuickForm {
    return new FormGroup({
      label: new FormControl(label, { nonNullable: true, validators: [Validators.required, Validators.maxLength(40)] }),
      query: new FormControl(query, { nonNullable: true, validators: [Validators.required] }),
    });
  }

  protected addColumn(): void {
    this.columns.push(this.columnForm('', [], null));
  }

  protected removeColumn(column: ColumnForm): void {
    this.columns.removeAt(this.columns.controls.indexOf(column));
  }

  /** Moves `column` one place up (-1) or down (1). */
  protected moveColumn(column: ColumnForm, step: -1 | 1): void {
    const from = this.columns.controls.indexOf(column);
    const to = from + step;
    if (from < 0 || to < 0 || to >= this.columns.length) return;
    this.columns.removeAt(from, { emitEvent: false });
    this.columns.insert(to, column);
  }

  protected addQuick(): void {
    this.quickFilters.push(this.quickForm('', ''));
  }

  protected removeQuick(quick: QuickForm): void {
    this.quickFilters.removeAt(this.quickFilters.controls.indexOf(quick));
  }

  /** The statuses another column already shows: offered, but marked. */
  protected takenElsewhere(column: ColumnForm, key: string): boolean {
    return this.columns.controls.some((c) => c !== column && c.controls.statuses.value.includes(key));
  }

  protected async save(): Promise<void> {
    this.form.markAllAsTouched();
    if (this.form.invalid || this.saving()) return;
    this.saving.set(true);
    this.errorKey.set(null);
    this.filterError.set(null);
    const value = this.form.getRawValue();
    const fields = {
      name: value.name.trim(),
      filter: value.filter.trim(),
      estimation: value.estimation,
      swimlanes: value.swimlanes,
    };
    try {
      let saved: Board;
      if (this.data.board) {
        saved = await this.api.invoke(updateBoard, {
          id: this.data.board.id,
          'If-Match': `"${this.data.board.version}"`,
          body: {
            ...fields,
            columns: value.columns.map((c) => ({ name: c.name.trim(), statuses: c.statuses, wipLimit: c.wipLimit || null })),
            quickFilters: value.quickFilters.map((q) => ({ label: q.label.trim(), query: q.query.trim() })),
          },
        });
      } else {
        saved = await this.api.invoke(createBoard, { body: { ...fields, nodeId: value.nodeId } });
      }
      this.dialogRef.close(saved);
    } catch (error) {
      this.showError(error);
    } finally {
      this.saving.set(false);
    }
  }

  private showError(error: unknown): void {
    if (!isApiProblem(error)) {
      this.errorKey.set(errorTranslationKey('unknown'));
      return;
    }
    if (error.query) {
      const message = queryErrorMessage(error.query, (key, params) => this.transloco.translate(key, params));
      this.filterError.set({ key: message.key, params: { ...message.params } });
      this.form.controls.filter.setErrors({ query: true });
      return;
    }
    if (error.status === 412) {
      this.errorKey.set('boards.settings.conflict');
      return;
    }
    let left = error.fieldErrors.length === 0;
    for (const fieldError of error.fieldErrors) {
      const control = this.form.get(fieldError.field);
      if (control) {
        control.setErrors({ api: fieldErrorTranslationKey(fieldError.code) });
        control.markAsTouched();
      } else {
        left = true;
      }
    }
    if (left) this.errorKey.set(error.translationKey);
  }

  protected delete(): void {
    const board = this.data.board;
    if (!board) return;
    const data: ConfirmDialogData = {
      titleKey: 'boards.settings.delete.title',
      messageKey: 'boards.settings.delete.message',
      confirmKey: 'boards.settings.delete.confirm',
      params: { name: board.name },
    };
    this.dialog
      .open(ConfirmDialog, { data })
      .afterClosed()
      .subscribe(async (confirmed?: boolean) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteBoard, { id: board.id, 'If-Match': `"${board.version}"` });
          this.dialogRef.close('deleted');
        } catch (error) {
          this.showError(error);
        }
      });
  }
}
