import { Component, computed, effect, inject, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { FormControl, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  BulkAction,
  BulkMovePlan,
  ErrorState,
  IssueMoveStatusNeed,
  IssueMoveStatusTarget,
  IssueMoveTypeNeed,
  IssueMoveTypeTarget,
  Node,
  NodeItem,
  createBulkAction,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  listNodes,
  planBulkMove,
} from 'ui';

import { Mappings, statusTargets, toBody, typeTargets } from '../issues/issue-move-dialog';
import { BulkDialogData } from './bulk-flow';

function isProject(item: NodeItem): item is Node & { key: string } {
  const node = item as Partial<Node>;
  return node.type === 'project' && !!node.key && node.archived === false;
}

/**
 * Moves the selected tickets to another project (L06-US08), as single moves
 * do (sub-tasks follow their parent, values the new types lack dropped),
 * with one plan for the batch: each type and status the target
 * configuration lacks mapped once, suggestions prefilled.
 */
@Component({
  selector: 'app-bulk-move-dialog',
  imports: [ErrorState, MatButtonModule, MatDialogModule, MatFormFieldModule, MatProgressBarModule, MatSelectModule, ReactiveFormsModule, TranslocoPipe],
  templateUrl: './bulk-move-dialog.html',
  styleUrl: './bulk-dialog.scss',
})
export class BulkMoveDialog {
  protected readonly data = inject<BulkDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<BulkMoveDialog, BulkAction>>(MatDialogRef);

  protected readonly project = new FormControl<string | null>(null, Validators.required);
  private readonly projectKey = toSignal(this.project.valueChanges, { initialValue: null });
  protected readonly projects = resource({
    loader: () => this.api.invoke(listNodes, { type: 'project', limit: 200 }),
  });
  protected readonly choices = computed(() => (this.projects.hasValue() ? this.projects.value().items.filter(isProject) : []));
  protected readonly types = signal<Mappings>({});
  protected readonly statuses = signal<Mappings>({});
  protected readonly plan = resource({
    params: () => {
      const projectKey = this.projectKey();
      return projectKey ? { projectKey, typeMappings: toBody(this.types()), statusMappings: toBody(this.statuses()) } : undefined;
    },
    loader: ({ params }) => this.api.invoke(planBulkMove, { body: { selection: this.data.selection, ...params } }),
  });
  protected readonly running = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    this.project.valueChanges.subscribe(() => {
      this.types.set({});
      this.statuses.set({});
    });
    // The suggestions fill what is not mapped yet.
    effect(() => {
      if (!this.plan.hasValue()) return;
      const plan = this.plan.value();
      const missing = (needs: readonly { id: string; suggestedId?: string | null }[], mapped: Mappings) =>
        needs.filter((n) => !(n.id in mapped) && n.suggestedId).map((n) => [n.id, n.suggestedId as string] as const);
      const types = missing(plan.types, this.types());
      if (types.length > 0) this.types.update((m) => ({ ...m, ...Object.fromEntries(types) }));
      const statuses = missing(plan.statuses, this.statuses());
      if (statuses.length > 0) this.statuses.update((m) => ({ ...m, ...Object.fromEntries(statuses) }));
    });
  }

  protected typeOptions(plan: BulkMovePlan, need: IssueMoveTypeNeed): IssueMoveTypeTarget[] {
    return typeTargets(plan, need, null);
  }

  protected statusOptions(plan: BulkMovePlan, need: IssueMoveStatusNeed): IssueMoveStatusTarget[] {
    return statusTargets(plan, need, this.types());
  }

  protected setType(id: string, target: string): void {
    this.types.update((m) => ({ ...m, [id]: target }));
  }

  protected setStatus(id: string, target: string): void {
    this.statuses.update((m) => ({ ...m, [id]: target }));
  }

  protected problemOf(plan: BulkMovePlan, path: string): string | null {
    const problem = plan.problems.find((p) => p.field === path);
    return problem ? fieldErrorTranslationKey(problem.code) : null;
  }

  protected labels(items: readonly { label?: string | null }[]): string {
    return items.map((i) => i.label ?? '').join(', ');
  }

  protected async run(): Promise<void> {
    const projectKey = this.projectKey();
    if (!projectKey || !this.plan.hasValue() || this.plan.value().problems.length > 0) return;
    this.running.set(true);
    this.errorKey.set(null);
    try {
      const done = await this.api.invoke(createBulkAction, {
        body: {
          selection: this.data.selection,
          action: { kind: 'move', projectKey, typeMappings: toBody(this.types()), statusMappings: toBody(this.statuses()) },
        },
      });
      this.dialogRef.close(done);
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.running.set(false);
    }
  }
}
