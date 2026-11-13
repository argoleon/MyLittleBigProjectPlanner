import { Component, Injectable, computed, effect, inject, input, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTabsModule } from '@angular/material/tabs';
import { Router, RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, ErrorState, Node, getNode } from 'ui';

import { ArchivedBanner } from './archived-banner';
import { NodeDialog, NodeDialogData } from './node-dialog';

/**
 * The node the settings page shows, shared with its tabs: they read it and
 * put back the version they saved.
 */
@Injectable()
export class NodeContext {
  readonly node = signal<Node | null>(null);
}

/** Tabs of a node's settings (mockup 14); "Configuration" comes with L04. */
const TABS = [
  { path: 'general', labelKey: 'hierarchy.settings.general' },
  { path: 'links', labelKey: 'hierarchy.settings.links' },
] as const;

/** Projects only. */
const ARCHIVING_TAB = { path: 'archiving', labelKey: 'hierarchy.settings.archiving' } as const;

/**
 * Settings of a portfolio, program or project (mockup 14): its parents and
 * its own view in the breadcrumb, "Add a child", and routed tabs. An
 * archived project shows a banner, and adds no child.
 */
@Component({
  selector: 'app-node-settings-page',
  imports: [ArchivedBanner, ErrorState, MatButtonModule, MatIconModule, MatProgressBarModule, MatTabsModule, RouterLink, RouterLinkActive, RouterOutlet, TranslocoPipe],
  providers: [NodeContext],
  styleUrl: './node-settings-page.scss',
  template: `
    @if (loaded.isLoading() && !context.node()) {
      <mat-progress-bar mode="indeterminate" [attr.aria-label]="'state.loading' | transloco" />
    } @else if (loaded.error()) {
      <ui-error-state [error]="loaded.error()" [headingLevel]="1" (retry)="loaded.reload()" />
    } @else if (context.node(); as node) {
      <nav class="eyebrow" [attr.aria-label]="'hierarchy.settings.breadcrumb' | transloco">
        <a routerLink="/portfolios">{{ 'nav.portfolios' | transloco }}</a>
        @for (parent of node.parents; track parent.id) {
          › <a [routerLink]="['/nodes', parent.id]">{{ parent.name }}</a>
        }
        › <a class="current" [routerLink]="['/nodes', node.id]">{{ node.name }}</a> › {{ 'hierarchy.view.settings' | transloco }}
      </nav>
      <div class="header">
        <h1 class="title">{{ 'hierarchy.settings.title' | transloco: { name: node.name } }}</h1>
        @if (!node.archived) {
          <button matButton="filled" type="button" class="add-child" (click)="addChild(node)">
            <mat-icon aria-hidden="true">add</mat-icon>
            {{ 'hierarchy.settings.addChild' | transloco }}
          </button>
        }
      </div>
      @if (node.archived) {
        <app-archived-banner />
      }
      <nav mat-tab-nav-bar [tabPanel]="panel" [attr.aria-label]="'hierarchy.settings.tabs' | transloco">
        @for (tab of tabs(); track tab.path) {
          <a mat-tab-link [routerLink]="tab.path" routerLinkActive #active="routerLinkActive" [active]="active.isActive">
            {{ tab.labelKey | transloco }}
          </a>
        }
      </nav>
      <mat-tab-nav-panel #panel class="panel">
        <router-outlet />
      </mat-tab-nav-panel>
    }
  `,
})
export class NodeSettingsPage {
  /** Id of the node, from the route. */
  readonly id = input.required<string>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);
  protected readonly context = inject(NodeContext);
  /** The Archiving tab is for projects. */
  protected readonly tabs = computed(() =>
    this.context.node()?.type === 'project' ? [...TABS, ARCHIVING_TAB] : [...TABS],
  );

  protected readonly loaded = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getNode, { id: params }),
  });

  constructor() {
    effect(() => {
      if (this.loaded.hasValue()) this.context.node.set(this.loaded.value());
    });
  }

  protected addChild(node: Node): void {
    const data: NodeDialogData = { parent: { id: node.id, type: node.type, name: node.name } };
    this.dialog
      .open<NodeDialog, NodeDialogData, Node>(NodeDialog, { data, width: 'min(560px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((child) => {
        // On to its settings, to complete it.
        if (child) void this.router.navigate(['/nodes', child.id, 'general']);
      });
  }
}
