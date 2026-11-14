import { Component, computed, effect, inject, resource, signal } from '@angular/core';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthService,
  ErrorState,
  ProjectConfig,
  TranslatedPipe,
  errorTranslationKey,
  getNodeConfiguration,
  isApiProblem,
  listProjectConfigs,
  setNodeConfiguration,
} from 'ui';

import { ConfigChips } from '../configuration/config-chips';
import { NodeContext } from './node-settings-page';

/**
 * "Configuration" tab of a project's settings (mockup 14): the project
 * configuration it uses, shared or not. Tenant admins and configuration
 * admins open it, or move the project to another one.
 */
@Component({
  selector: 'app-node-configuration-tab',
  imports: [
    ConfigChips,
    ErrorState,
    MatButtonModule,
    MatFormFieldModule,
    MatProgressBarModule,
    MatSelectModule,
    ReactiveFormsModule,
    RouterLink,
    TranslatedPipe,
    TranslocoPipe,
  ],
  styleUrl: './node-configuration-tab.scss',
  templateUrl: './node-configuration-tab.html',
})
export class NodeConfigurationTab {
  private readonly api = inject(Api);
  private readonly auth = inject(AuthService);
  private readonly context = inject(NodeContext);

  protected readonly node = this.context.node;
  private readonly id = computed(() => this.node()?.id);
  protected readonly current = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getNodeConfiguration, { id: params }),
  });
  /** Tenant admins and configuration admins manage configurations. */
  protected readonly manages = computed(() =>
    ['tenant_admin', 'configuration_admin'].includes(this.auth.session()?.account.tenantRole ?? ''),
  );
  /** The configurations to move to, for those who manage them. */
  protected readonly configs = resource({
    params: () => (this.manages() ? true : undefined),
    loader: async () => {
      const all: ProjectConfig[] = [];
      let cursor: string | undefined;
      do {
        const page = await this.api.invoke(listProjectConfigs, { limit: 200, cursor });
        all.push(...page.items);
        cursor = page.nextCursor ?? undefined;
      } while (cursor);
      return all;
    },
  });
  protected readonly choice = new FormControl<string | null>(null);
  protected readonly saving = signal(false);
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);

  constructor() {
    effect(() => {
      if (this.current.hasValue()) this.choice.setValue(this.current.value().id);
    });
    effect(() => {
      if (this.node()?.archived) this.choice.disable();
      else this.choice.enable();
    });
  }

  protected async move(): Promise<void> {
    const node = this.node();
    const configId = this.choice.value;
    if (!node || !configId || configId === (this.current.hasValue() ? this.current.value().id : null)) return;
    this.saving.set(true);
    this.notice.set(null);
    try {
      const moved = await this.api.invoke(setNodeConfiguration, { id: node.id, body: { configId } });
      this.current.set(moved);
      this.configs.reload();
      this.notice.set({ ok: true, key: 'config.node.moved', params: { name: moved.name ?? '' } });
    } catch (error) {
      this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.saving.set(false);
    }
  }
}
