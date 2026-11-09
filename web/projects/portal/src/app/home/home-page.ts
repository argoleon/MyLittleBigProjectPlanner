import { Component } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';
import { EmptyState } from 'ui';

/** Placeholder home page; request types and forms come with L11. */
@Component({
  selector: 'portal-home-page',
  imports: [EmptyState, TranslocoPipe],
  styleUrl: './home-page.scss',
  template: `
    <h1 class="title">{{ 'page.portalHome.title' | transloco }}</h1>
    <ui-empty-state icon="support_agent" [title]="'page.comingSoon.title' | transloco" [message]="'page.portalHome.message' | transloco" />
  `,
})
export class HomePage {}
