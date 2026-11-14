import { ComponentType } from '@angular/cdk/portal';
import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  ErrorState,
  StatusChip,
  TranslatedPipe,
  Workflow,
  WorkflowList,
  WorkflowProblem,
  WorkflowStatus,
  WorkflowTransition,
  deleteWorkflow,
  deleteWorkflowStatus,
  deleteWorkflowTransition,
  errorTranslationKey,
  getProjectConfig,
  isApiProblem,
  listFields,
  listIssueTypes,
  listScreens,
  LocaleService,
  listWorkflows,
  resolveLabel,
} from 'ui';

import { ConfigContext } from './config-page';
import { WorkflowGraph } from './workflow-graph';
import {
  StatusDialog,
  StatusDialogData,
  TransitionDialog,
  TransitionDialogData,
  WorkflowDialog,
  WorkflowDialogData,
} from './workflow-dialogs';
import { chipCategory } from './workflow-layout';
import { Summary, conditionNames, postFunctionSummaries } from './transition-rules';

/**
 * "Workflows" tab of a configuration (mockup 11, L04-US04): the workflows
 * with the issue types that follow them; for the one chosen, what keeps it
 * from being published, its read-only graph, and its statuses and
 * transitions in tables, edited in dialogs. Changes go to the draft.
 */
@Component({
  selector: 'app-workflows-tab',
  imports: [ErrorState, MatButtonModule, MatIconModule, MatProgressBarModule, MatTableModule, StatusChip, TranslatedPipe, TranslocoPipe, WorkflowGraph],
  styleUrl: './workflows-tab.scss',
  templateUrl: './workflows-tab.html',
})
export class WorkflowsTab {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);
  protected readonly context = inject(ConfigContext);

  protected readonly statusColumns = ['label', 'key', 'category', 'portal', 'actions'];
  protected readonly transitionColumns = ['name', 'move', 'rules', 'actions'];
  protected readonly configId = computed(() => this.context.config()?.id);
  protected readonly workflows = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listWorkflows, { id: params }),
  });
  private readonly types = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listIssueTypes, { id: params }),
  });
  protected readonly items = computed(() => (this.workflows.hasValue() ? this.workflows.value().items : []));
  /** The names of the roles, groups and members the transition rules name. */
  private readonly references = computed(() => (this.workflows.hasValue() ? this.workflows.value().references : null));
  private readonly fieldList = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listFields, { id: params }),
  });
  private readonly fields = computed(() => (this.fieldList.hasValue() ? this.fieldList.value().items : []));
  private readonly screenList = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listScreens, { id: params }),
  });
  /** The transition screens a transition may show. */
  private readonly transitionScreens = computed(() =>
    this.screenList.hasValue() ? this.screenList.value().items.filter((s) => s.kind === 'transition') : [],
  );

  /** The name of the transition screen of `transition`, if any. */
  protected screenOf(transition: WorkflowTransition): string | null {
    const screen = this.transitionScreens().find((s) => s.id === transition.rules.screenId);
    return screen ? resolveLabel(screen.nameTranslations, this.locale.language()) : null;
  }
  private readonly chosen = signal<string | null>(null);
  /** The workflow shown: the one chosen, else the first. */
  protected readonly selected = computed(() => this.items().find((w) => w.id === this.chosen()) ?? this.items()[0] ?? null);
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);
  protected readonly chipCategory = chipCategory;

  protected choose(workflow: Workflow): void {
    this.chosen.set(workflow.id);
  }

  /** The labels of the issue types that follow `workflow`. */
  protected typesOf(workflow: Workflow): string {
    const types = this.types.hasValue() ? this.types.value().items : [];
    return types
      .filter((t) => workflow.issueTypeIds.includes(t.id))
      .map((t) => resolveLabel(t.labelTranslations, this.locale.language()))
      .join(', ');
  }

  /** Who may take `transition`, by name; empty: everyone. A role or group deleted since is said so. */
  protected restrictedTo(transition: WorkflowTransition): string[] {
    const { roles, groups } = conditionNames(transition.rules, this.references(), (labels) =>
      resolveLabel(labels, this.locale.language()),
    );
    return [...roles, ...groups].map((name) => name ?? '');
  }

  /** The fields `transition` requires, by label. */
  protected requiredFields(transition: WorkflowTransition): string[] {
    return (transition.rules.validators ?? []).map((id) => this.fieldName(id));
  }

  protected postFunctions(transition: WorkflowTransition): Summary[] {
    return postFunctionSummaries(transition.rules, this.references(), (id) => this.fieldName(id));
  }

  private fieldName(id: string | null | undefined): string {
    const field = this.fields().find((f) => f.id === id);
    return field ? resolveLabel(field.labelTranslations, this.locale.language()) : '';
  }

  protected statusLabel(workflow: Workflow, id: string | null): string {
    const status = workflow.statuses.find((s) => s.id === id);
    return status ? resolveLabel(status.labelTranslations, this.locale.language()) : '';
  }

  /** What a problem says, naming its status or transition. */
  protected problemParams(workflow: Workflow, problem: WorkflowProblem): Record<string, string> {
    const transition = workflow.transitions.find((t) => t.id === problem.transitionId);
    return {
      status: this.statusLabel(workflow, problem.statusId),
      transition: transition ? resolveLabel(transition.nameTranslations, this.locale.language()) : '',
    };
  }

  protected addWorkflow(): void {
    const data: WorkflowDialogData = { configId: this.configId()!, workflows: this.items() };
    this.open(WorkflowDialog, data, 'config.workflows.saved');
  }

  protected renameWorkflow(workflow: Workflow): void {
    const data: WorkflowDialogData = { configId: this.configId()!, workflow, workflows: this.items() };
    this.open(WorkflowDialog, data, 'config.workflows.saved');
  }

  protected editStatus(workflow: Workflow, status?: WorkflowStatus): void {
    const data: StatusDialogData = { configId: this.configId()!, workflow, status };
    this.open(StatusDialog, data, 'config.workflows.status.saved');
  }

  protected editTransition(workflow: Workflow, transition?: WorkflowTransition): void {
    const data: TransitionDialogData = {
      configId: this.configId()!,
      workflow,
      transition,
      fields: this.fields(),
      references: this.references(),
      screens: this.transitionScreens(),
    };
    // Its rules may name roles, groups or members the list does not name yet.
    this.open(TransitionDialog, data, 'config.workflows.transition.saved', { width: 'min(720px, 100vw)', reload: true });
  }

  protected removeWorkflow(workflow: Workflow): void {
    this.confirm('config.workflows.remove.', resolveLabel(workflow.labelTranslations, this.locale.language()), () =>
      this.api.invoke(deleteWorkflow, { id: this.configId()!, workflowId: workflow.id, 'If-Match': `"${workflow.version}"` }),
    );
  }

  protected removeStatus(status: WorkflowStatus): void {
    this.confirm('config.workflows.status.remove.', resolveLabel(status.labelTranslations, this.locale.language()), () =>
      this.api.invoke(deleteWorkflowStatus, { id: this.configId()!, statusId: status.id, 'If-Match': `"${status.version}"` }),
    );
  }

  protected removeTransition(transition: WorkflowTransition): void {
    this.confirm('config.workflows.transition.remove.', resolveLabel(transition.nameTranslations, this.locale.language()), () =>
      this.api.invoke(deleteWorkflowTransition, {
        id: this.configId()!,
        transitionId: transition.id,
        'If-Match': `"${transition.version}"`,
      }),
    );
  }

  private open<C, D>(
    component: ComponentType<C>,
    data: D,
    noticeKey: string,
    options: { width?: string; reload?: boolean } = {},
  ): void {
    this.dialog
      .open<C, D, Workflow>(component, { data, width: options.width ?? 'min(560px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe(async (saved) => {
        if (!saved) return;
        this.chosen.set(saved.id);
        this.notice.set({ ok: true, key: noticeKey, params: { name: resolveLabel(saved.labelTranslations, this.locale.language()) } });
        this.replace(saved);
        if (options.reload) this.workflows.reload();
        await this.refreshConfig();
      });
  }

  /** The workflow saved in place of its former self, or added. */
  private replace(saved: Workflow): void {
    if (!this.workflows.hasValue()) return;
    const list: WorkflowList = this.workflows.value();
    const found = list.items.some((w) => w.id === saved.id);
    const items = found ? list.items.map((w) => (w.id === saved.id ? saved : w)) : [...list.items, saved];
    this.workflows.set({ stage: 'draft', items, references: list.references });
  }

  /** Asks to confirm a removal; `base` is the prefix of its keys (`config.workflows.remove.`). */
  private confirm(base: string, name: string, remove: () => Promise<unknown>): void {
    const data: ConfirmDialogData = {
      titleKey: `${base}title`,
      messageKey: `${base}message`,
      confirmKey: `${base}confirm`,
      params: { name },
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await remove();
          this.notice.set({ ok: true, key: `${base}done`, params: { name } });
          this.workflows.reload();
          this.types.reload();
          await this.refreshConfig();
        } catch (error) {
          this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }

  private async refreshConfig(): Promise<void> {
    const id = this.configId();
    if (id) this.context.config.set(await this.api.invoke(getProjectConfig, { id }));
  }
}
