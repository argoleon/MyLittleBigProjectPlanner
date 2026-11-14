import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfigField,
  ErrorState,
  IssueType,
  TranslatedPipe,
  getProjectConfig,
  listFields,
  listIssueTypes,
  listWorkflows,
} from 'ui';

import { ConfigContext } from './config-page';
import { FieldEditor } from './field-editor';
import { kindIcon, languagesOf, typesOf } from './fields';

/** What the panel shows: a field to change, or a new one. */
interface Editing {
  readonly field: ConfigField | null;
}

/**
 * "Fields" tab of a configuration (L04-US03, mockup 12): its system and
 * custom fields with their identifier, type, the issue types they apply
 * to and the languages of their label; edited in a panel with their rules
 * per type and workflow step. Changes go to the draft.
 */
@Component({
  selector: 'app-fields-tab',
  imports: [ErrorState, FieldEditor, MatButtonModule, MatIconModule, MatProgressBarModule, MatTableModule, TranslatedPipe, TranslocoPipe],
  styleUrl: './fields-tab.scss',
  templateUrl: './fields-tab.html',
})
export class FieldsTab {
  private readonly api = inject(Api);
  protected readonly context = inject(ConfigContext);

  protected readonly columns = ['field', 'key', 'kind', 'types', 'languages', 'actions'];
  protected readonly configId = computed(() => this.context.config()?.id);
  protected readonly fields = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listFields, { id: params }),
  });
  protected readonly items = computed(() => (this.fields.hasValue() ? this.fields.value().items : []));
  private readonly typeList = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listIssueTypes, { id: params }),
  });
  protected readonly types = computed(() => (this.typeList.hasValue() ? this.typeList.value().items : []));
  private readonly workflowList = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listWorkflows, { id: params }),
  });
  protected readonly workflows = computed(() => (this.workflowList.hasValue() ? this.workflowList.value().items : []));
  protected readonly editing = signal<Editing | null>(null);
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);
  protected readonly kindIcon = kindIcon;
  protected readonly languagesOf = languagesOf;

  /** The types of `field`, or null for all of them. */
  protected typesOf(field: ConfigField): IssueType[] | null {
    return typesOf(field, this.types());
  }

  protected edit(field: ConfigField | null): void {
    this.notice.set(null);
    this.editing.set({ field });
  }

  protected close(): void {
    this.editing.set(null);
  }

  protected async saved(field: ConfigField): Promise<void> {
    this.editing.set({ field });
    this.notice.set({ ok: true, key: 'config.fields.saved', params: { name: field.label ?? field.key } });
    await this.refresh();
  }

  protected async removed(field: ConfigField): Promise<void> {
    this.editing.set(null);
    this.notice.set({ ok: true, key: 'config.fields.removed', params: { name: field.label ?? field.key } });
    await this.refresh();
  }

  /** The fields again, and the configuration's draft chip. */
  private async refresh(): Promise<void> {
    this.fields.reload();
    const id = this.configId();
    if (id) this.context.config.set(await this.api.invoke(getProjectConfig, { id }));
  }
}
