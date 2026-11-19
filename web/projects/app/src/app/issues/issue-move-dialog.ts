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
  ErrorState,
  Issue,
  IssueMoveMapping,
  IssueMovePlan,
  IssueMoveStatusNeed,
  IssueMoveStatusTarget,
  IssueMoveTypeNeed,
  IssueMoveTypeTarget,
  Node,
  NodeItem,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  listNodes,
  moveIssue,
  planIssueMove,
} from 'ui';

/** How the move dialog closes: the ticket moved, or a reload asked for after a conflict. */
export type MoveOutcome = { readonly moved: Issue } | { readonly reload: true };

export type Mappings = Readonly<Record<string, string>>;

/** The mappings to send. */
export function toBody(mappings: Mappings): IssueMoveMapping[] {
  return Object.entries(mappings).map(([fromId, toId]) => ({ fromId, toId }));
}

/** The target types a type need may map to: the ticket's own to a type that is no sub-task, a sub-task's to a sub-task type allowed under it. */
export function typeTargets(
  plan: Pick<IssueMovePlan, 'targets'>,
  need: IssueMoveTypeNeed,
  rootTarget: string | null,
): IssueMoveTypeTarget[] {
  return plan.targets.filter((t) =>
    need.isSubtask ? t.isSubtask && (rootTarget === null || t.parentIds.includes(rootTarget)) : !t.isSubtask,
  );
}

/** The target statuses a status need may map to: those of the workflows of its tickets' new types. */
export function statusTargets(
  plan: Pick<IssueMovePlan, 'targets' | 'statusTargets'>,
  need: IssueMoveStatusNeed,
  types: Mappings,
): IssueMoveStatusTarget[] {
  const ids = new Set(
    need.typeIds.flatMap((typeId) => plan.targets.find((t) => t.id === (types[typeId] ?? typeId))?.statusIds ?? []),
  );
  return plan.statusTargets.filter((s) => ids.has(s.id));
}

function isProject(item: NodeItem): item is Node & { key: string } {
  const node = item as Partial<Node>;
  return node.type === 'project' && !!node.key && node.archived === false;
}

/**
 * Moves a ticket to another project (L05-US02), its sub-tasks with it:
 * the target project, then the types and statuses its configuration lacks
 * mapped (suggestions prefilled), the values dropped and what would be
 * refused, all from the plan the API makes; the move carries the version
 * read.
 */
@Component({
  selector: 'app-issue-move-dialog',
  imports: [ErrorState, MatButtonModule, MatDialogModule, MatFormFieldModule, MatProgressBarModule, MatSelectModule, ReactiveFormsModule, TranslocoPipe],
  templateUrl: './issue-move-dialog.html',
  styleUrl: './issue-dialog.scss',
})
export class IssueMoveDialog {
  protected readonly ticket = inject<Issue>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<IssueMoveDialog, MoveOutcome>>(MatDialogRef);

  protected readonly project = new FormControl<string | null>(null, Validators.required);
  private readonly projectKey = toSignal(this.project.valueChanges, { initialValue: null });
  protected readonly projects = resource({
    loader: () => this.api.invoke(listNodes, { type: 'project', limit: 200 }),
  });
  protected readonly choices = computed(() =>
    this.projects.hasValue() ? this.projects.value().items.filter(isProject).filter((p) => p.key !== this.ticket.project.key) : [],
  );
  /** Types and statuses mapped, by id of the ticket's configuration. */
  protected readonly types = signal<Mappings>({});
  protected readonly statuses = signal<Mappings>({});
  protected readonly plan = resource({
    params: () => {
      const projectKey = this.projectKey();
      return projectKey ? { projectKey, typeMappings: toBody(this.types()), statusMappings: toBody(this.statuses()) } : undefined;
    },
    loader: ({ params }) => this.api.invoke(planIssueMove, { idOrKey: this.ticket.id, body: params }),
  });
  /** The type the ticket itself lands on. */
  protected readonly rootTarget = computed(() => this.types()[this.ticket.type.id] ?? this.ticket.type.id);
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  protected readonly conflict = signal(false);

  constructor() {
    // Another project: its own mappings.
    this.project.valueChanges.subscribe(() => {
      this.types.set({});
      this.statuses.set({});
    });
    // The suggestions fill what is not mapped yet.
    effect(() => {
      if (!this.plan.hasValue()) return;
      const plan = this.plan.value();
      const missing = (needs: readonly { id: string; suggestedId: string | null }[], mapped: Mappings) =>
        needs.filter((n) => !(n.id in mapped) && n.suggestedId).map((n) => [n.id, n.suggestedId as string] as const);
      const types = missing(plan.types, this.types());
      if (types.length > 0) this.types.update((m) => ({ ...m, ...Object.fromEntries(types) }));
      const statuses = missing(plan.statuses, this.statuses());
      if (statuses.length > 0) this.statuses.update((m) => ({ ...m, ...Object.fromEntries(statuses) }));
    });
  }

  protected typeOptions(plan: IssueMovePlan, need: IssueMoveTypeNeed): IssueMoveTypeTarget[] {
    return typeTargets(plan, need, need.isSubtask ? this.rootTarget() : null);
  }

  protected statusOptions(plan: IssueMovePlan, need: IssueMoveStatusNeed): IssueMoveStatusTarget[] {
    return statusTargets(plan, need, this.types());
  }

  protected setType(id: string, target: string): void {
    this.types.update((m) => ({ ...m, [id]: target }));
  }

  protected setStatus(id: string, target: string): void {
    this.statuses.update((m) => ({ ...m, [id]: target }));
  }

  /** The translation key of the problem the move would have with a mapping, if any. */
  protected problemOf(plan: IssueMovePlan, path: string): string | null {
    const problem = plan.problems.find((p) => p.field === path);
    return problem ? fieldErrorTranslationKey(problem.code) : null;
  }

  protected labels(items: readonly { label?: string | null }[]): string {
    return items.map((i) => i.label ?? '').join(', ');
  }

  protected reload(): void {
    this.dialogRef.close({ reload: true });
  }

  protected async move(): Promise<void> {
    const projectKey = this.projectKey();
    if (!projectKey || !this.plan.hasValue() || this.plan.value().problems.length > 0) return;
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const moved = await this.api.invoke(moveIssue, {
        idOrKey: this.ticket.id,
        'If-Match': `"${this.ticket.version}"`,
        body: { projectKey, typeMappings: toBody(this.types()), statusMappings: toBody(this.statuses()) },
      });
      this.dialogRef.close({ moved });
    } catch (error) {
      if (isApiProblem(error) && error.status === 412) {
        this.conflict.set(true);
      } else {
        this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      }
    } finally {
      this.saving.set(false);
    }
  }
}
