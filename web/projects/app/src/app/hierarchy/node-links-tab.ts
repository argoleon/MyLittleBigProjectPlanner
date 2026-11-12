import { Component, ElementRef, Injector, afterNextRender, computed, inject, resource, signal, viewChild } from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime } from 'rxjs';
import {
  Api,
  ErrorState,
  Node,
  NodeCandidate,
  NodeLink,
  errorTranslationKey,
  getNode,
  isApiProblem,
  linkNodeParent,
  listNodeChildren,
  listNodeParentCandidates,
  listNodeParents,
  unlinkNodeParent,
} from 'ui';

import { NODE_ICONS, parentNames } from './node-links';
import { NodeContext } from './node-settings-page';
import { WeightField } from './weight-field';

/** Candidate parents shown at most. */
const CANDIDATES = 20;

interface Notice {
  ok: boolean;
  key: string;
  params?: Record<string, string>;
}

/**
 * "Attachments" tab of a node's settings (mockup 14): its parents with the
 * weighting of each link, a search to add a parent that says why a node
 * cannot be one, and its children.
 */
@Component({
  selector: 'app-node-links-tab',
  imports: [
    ErrorState,
    MatButtonModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatProgressBarModule,
    RouterLink,
    TranslocoPipe,
    WeightField,
  ],
  templateUrl: './node-links-tab.html',
  styleUrl: './node-links-tab.scss',
})
export class NodeLinksTab {
  private readonly api = inject(Api);
  private readonly context = inject(NodeContext);
  private readonly injector = inject(Injector);

  protected readonly node = this.context.node;
  protected readonly icons = NODE_ICONS;
  protected readonly parentNames = parentNames;
  private readonly id = computed(() => this.node()?.id);

  protected readonly parents = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(listNodeParents, { id: params }),
  });
  protected readonly children = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(listNodeChildren, { id: params }),
  });

  /** Whether the "Add a parent" search is open. */
  protected readonly searching = signal(false);
  protected readonly query = signal('');
  private readonly debounced = toSignal(toObservable(this.query).pipe(debounceTime(250)), { initialValue: '' });
  protected readonly candidates = resource({
    params: () => {
      const id = this.id();
      return this.searching() && id ? { id, search: this.debounced().trim() || undefined } : undefined;
    },
    loader: ({ params }) => this.api.invoke(listNodeParentCandidates, { ...params, limit: CANDIDATES }),
  });

  protected readonly busy = signal(false);
  protected readonly notice = signal<Notice | null>(null);

  private readonly addButton = viewChild('addButton', { read: ElementRef<HTMLButtonElement> });
  private readonly searchInput = viewChild('searchInput', { read: ElementRef<HTMLInputElement> });

  protected openSearch(): void {
    this.searching.set(true);
    afterNextRender(() => this.searchInput()?.nativeElement.focus(), { injector: this.injector });
  }

  protected closeSearch(): void {
    this.searching.set(false);
    this.query.set('');
    this.addButton()?.nativeElement.focus();
  }

  protected async link(node: Node, candidate: NodeCandidate): Promise<void> {
    const done = await this.run(
      () => this.api.invoke(linkNodeParent, { id: node.id, parentId: candidate.id, body: { weightPercent: 100 } }),
      { ok: true, key: 'hierarchy.links.linked', params: { name: candidate.name } },
    );
    if (!done) return;
    this.candidates.reload();
    // The button became "already a parent": back to the search.
    this.searchInput()?.nativeElement.focus();
  }

  protected async setWeight(node: Node, link: NodeLink, weight: number): Promise<void> {
    await this.run(
      () => this.api.invoke(linkNodeParent, { id: node.id, parentId: link.id, body: { weightPercent: weight } }),
      { ok: true, key: 'hierarchy.links.weightSaved', params: { name: link.name } },
    );
  }

  protected async unlink(node: Node, link: NodeLink): Promise<void> {
    const done = await this.run(
      () => this.api.invoke(unlinkNodeParent, { id: node.id, parentId: link.id }),
      { ok: true, key: 'hierarchy.links.unlinked', params: { name: link.name } },
    );
    if (!done) return;
    if (this.searching()) this.candidates.reload();
    // The row and its button are gone.
    this.addButton()?.nativeElement.focus();
  }

  /** Runs a change of links, then reloads them and the node (its breadcrumb). */
  private async run(change: () => Promise<unknown>, success: Notice): Promise<boolean> {
    const node = this.node();
    if (!node) return false;
    this.busy.set(true);
    this.notice.set(null);
    try {
      await change();
      this.notice.set(success);
      this.parents.reload();
      this.context.node.set(await this.api.invoke(getNode, { id: node.id }));
      return true;
    } catch (error) {
      this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
      this.parents.reload();
      return false;
    } finally {
      this.busy.set(false);
    }
  }
}
