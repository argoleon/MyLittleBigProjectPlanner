import { Component, computed, input } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';
import { TranslatedPipe, Workflow } from 'ui';

import { NODE, layoutWorkflow } from './workflow-layout';

/**
 * The read-only graph of a workflow (mockup 11): its statuses colored by
 * category, its transitions as labelled arrows. An image with a name;
 * the statuses and transitions tables below it are its text alternative.
 */
@Component({
  selector: 'app-workflow-graph',
  imports: [TranslatedPipe, TranslocoPipe],
  template: `
    <svg
      role="img"
      [attr.aria-label]="'config.workflows.graph' | transloco: { name: workflow().labelTranslations | translated }"
      [attr.viewBox]="'0 0 ' + layout().width + ' ' + layout().height"
      [attr.width]="layout().width"
      [attr.height]="layout().height"
    >
      <defs>
        <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M 0 0 L 10 5 L 0 10 z" class="arrow-head" />
        </marker>
      </defs>
      @for (edge of layout().transitions; track edge.transition.id) {
        <path [attr.d]="edge.path" class="edge" [class.any]="edge.fromAny" marker-end="url(#arrow)" />
        <text [attr.x]="edge.labelX" [attr.y]="edge.labelY" class="edge-label" [attr.text-anchor]="edge.fromAny ? 'start' : 'middle'">
          {{ edge.transition.nameTranslations | translated }}
        </text>
      }
      @for (node of layout().statuses; track node.status.id) {
        <g [attr.transform]="'translate(' + node.x + ' ' + node.y + ')'" [attr.data-category]="node.category" [class.apart]="node.apart">
          <rect [attr.width]="width" [attr.height]="height" rx="12" class="node" />
          <text [attr.x]="width / 2" [attr.y]="height / 2" class="node-label" text-anchor="middle" dominant-baseline="central">
            {{ node.status.labelTranslations | translated }}
          </text>
        </g>
      }
    </svg>
  `,
  styleUrl: './workflow-graph.scss',
  // A wide graph scrolls: reachable with the keyboard.
  host: { tabindex: '0' },
})
export class WorkflowGraph {
  readonly workflow = input.required<Pick<Workflow, 'statuses' | 'transitions' | 'labelTranslations'>>();

  protected readonly layout = computed(() => layoutWorkflow(this.workflow()));
  protected readonly width = NODE.width;
  protected readonly height = NODE.height;
}
