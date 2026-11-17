import { CdkDrag, CdkDragDrop, CdkDragHandle, CdkDropList } from '@angular/cdk/drag-drop';
import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ErrorState,
  IssueType,
  LocalNumberPipe,
  TranslatedPipe,
  TypeChip,
  errorTranslationKey,
  getProjectConfig,
  isApiProblem,
  listIssueTypes,
  listWorkflows,
  reorderIssueTypes,
} from 'ui';

import { reordered } from './issue-types';
import { ConfigContext } from './config-page';
import { TypeEditor } from './type-editor';

/** What the panel shows: a type to change, or a new one. */
interface Editing {
  readonly type: IssueType | null;
}

/**
 * "Types" tab of a configuration (L04-US02): its issue types in their
 * order, with their identifier, the types they may sit under and their
 * level; reordered by drag and drop or with the keyboard, edited in a
 * panel. Changes go to the draft.
 */
@Component({
  selector: 'app-types-tab',
  imports: [
    CdkDrag,
    CdkDragHandle,
    CdkDropList,
    ErrorState,
    LocalNumberPipe,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatTableModule,
    TranslatedPipe,
    TranslocoPipe,
    TypeChip,
    TypeEditor,
  ],
  styleUrl: './types-tab.scss',
  templateUrl: './types-tab.html',
})
export class TypesTab {
  private readonly api = inject(Api);
  protected readonly context = inject(ConfigContext);

  protected readonly columns = ['handle', 'type', 'key', 'parents', 'level', 'actions'];
  protected readonly configId = computed(() => this.context.config()?.id);
  protected readonly types = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listIssueTypes, { id: params }),
  });
  protected readonly items = computed(() => (this.types.hasValue() ? this.types.value().items : []));
  /** The workflows a type may follow, for the panel. */
  private readonly workflowList = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listWorkflows, { id: params }),
  });
  protected readonly workflows = computed(() => (this.workflowList.hasValue() ? this.workflowList.value().items : []));
  protected readonly editing = signal<Editing | null>(null);
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);
  protected readonly moving = signal(false);

  /** The labels of the parents of `type`, in the user's language. */
  protected parentsOf(type: IssueType): IssueType[] {
    return this.items().filter((other) => type.parentIds.includes(other.id));
  }

  protected edit(type: IssueType | null): void {
    this.notice.set(null);
    this.editing.set({ type });
  }

  protected close(): void {
    this.editing.set(null);
  }

  protected async saved(type: IssueType): Promise<void> {
    this.editing.set({ type });
    this.notice.set({ ok: true, key: 'config.types.saved', params: { name: type.label ?? type.key } });
    await this.refresh();
  }

  protected async removed(type: IssueType): Promise<void> {
    this.editing.set(null);
    this.notice.set({ ok: true, key: 'config.types.removed', params: { name: type.label ?? type.key } });
    await this.refresh();
  }

  protected drop(event: CdkDragDrop<IssueType[]>): void {
    if (event.previousIndex !== event.currentIndex) void this.move(event.previousIndex, event.currentIndex);
  }

  /**
   * Moves `type` one place up (-1) or down (1): the keyboard alternative to
   * dragging. Its position is read now: the one the list shows may be a
   * change behind (two quick presses).
   */
  protected shift(type: IssueType, by: -1 | 1): void {
    const from = this.items().findIndex((t) => t.id === type.id);
    if (from >= 0) void this.move(from, from + by);
  }

  /**
   * Moves the type at `from` to `to`, one move at a time: a press during a
   * move, before its button shows as disabled, would reorder the list as it
   * was before.
   */
  protected async move(from: number, to: number): Promise<void> {
    const id = this.configId();
    if (!id || this.moving() || to < 0 || to >= this.items().length) return;
    const moved = this.items()[from];
    this.moving.set(true);
    try {
      const list = await this.api.invoke(reorderIssueTypes, { id, body: { ids: reordered(this.items(), from, to) } });
      this.types.set(list);
      this.notice.set({
        ok: true,
        key: 'config.types.moved',
        params: { name: moved.label ?? moved.key, position: String(to + 1) },
      });
      await this.refreshConfig(id);
    } catch (error) {
      this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.moving.set(false);
    }
  }

  /** The types again, and the configuration's draft chip. */
  private async refresh(): Promise<void> {
    this.types.reload();
    const id = this.configId();
    if (id) await this.refreshConfig(id);
  }

  private async refreshConfig(id: string): Promise<void> {
    this.context.config.set(await this.api.invoke(getProjectConfig, { id }));
  }
}
