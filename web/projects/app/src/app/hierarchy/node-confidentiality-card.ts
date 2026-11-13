import { Component, inject, input, signal } from '@angular/core';
import { MatSlideToggleChange, MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Node, errorTranslationKey, isApiProblem, setNodeConfidentiality } from 'ui';

import { NodeContext } from './node-settings-page';

/**
 * "Confidentiality" card of a project (mockup 14): a switch that blocks
 * inherited access, so that only the roles granted on the project itself
 * reach it. Saved at once, with the version of the node it was read at.
 */
@Component({
  selector: 'app-node-confidentiality-card',
  imports: [MatSlideToggleModule, TranslocoPipe],
  templateUrl: './node-confidentiality-card.html',
  styleUrl: './node-confidentiality-card.scss',
})
export class NodeConfidentialityCard {
  private readonly api = inject(Api);
  private readonly context = inject(NodeContext);

  /** The project. */
  readonly node = input.required<Node>();

  protected readonly saving = signal(false);
  protected readonly notice = signal<{ ok: boolean; key: string } | null>(null);

  protected async set(change: MatSlideToggleChange): Promise<void> {
    const node = this.node();
    this.saving.set(true);
    this.notice.set(null);
    try {
      const saved = await this.api.invoke(setNodeConfidentiality, {
        id: node.id,
        'If-Match': `"${node.version}"`,
        body: { confidential: change.checked },
      });
      // The settings follow: the access of the node is read again.
      this.context.node.set(saved);
      this.notice.set({ ok: true, key: saved.confidential ? 'hierarchy.confidentiality.on' : 'hierarchy.confidentiality.off' });
    } catch (error) {
      // Refused: the switch goes back to what is saved.
      change.source.checked = node.confidential;
      this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.saving.set(false);
    }
  }
}
