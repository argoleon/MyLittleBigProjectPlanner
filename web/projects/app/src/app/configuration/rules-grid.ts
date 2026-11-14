import { Component, computed, input, model } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';
import { FieldMode, FieldRule, IssueType, TranslatedPipe, Translations, Workflow, WorkflowStatus } from 'ui';

import { Applicability, cycled, modeOf, rulesGrids } from './fields';

/**
 * The rules of a field (mockup 12): one grid per workflow, a row per type
 * of the field that follows it, a column for the creation and each status.
 * Each cell is a button that cycles Hidden > Visible > Required; a cell
 * nobody set is Visible.
 */
@Component({
  selector: 'app-rules-grid',
  imports: [TranslatedPipe, TranslocoPipe],
  template: `
    @for (grid of grids(); track grid.workflow.id) {
      <div class="grid">
        <table class="rules">
          <caption>
            {{ 'config.fields.rules.caption' | transloco: { workflow: grid.workflow.labelTranslations | translated } }}
          </caption>
          <thead>
            <tr>
              <th scope="col">{{ 'config.fields.rules.type' | transloco }}</th>
              <th scope="col">{{ 'config.fields.rules.creation' | transloco }}</th>
              @for (status of grid.statuses; track status.id) {
                <th scope="col">{{ status.labelTranslations | translated }}</th>
              }
            </tr>
          </thead>
          <tbody>
            @for (type of grid.types; track type.id) {
              @let typeLabel = type.labelTranslations | translated;
              <tr>
                <th scope="row">{{ typeLabel }}</th>
                @for (column of columnsOf(grid.statuses); track column.statusId) {
                  @let mode = modeOf(type.id, column.statusId);
                  @let modeLabel = 'config.fields.modes.' + mode | transloco;
                  @let step = column.labels ? (column.labels | translated) : ('config.fields.rules.creation' | transloco);
                  <td>
                    <button
                      type="button"
                      class="mode"
                      [attr.data-mode]="mode"
                      [disabled]="disabled()"
                      (click)="cycle(type.id, column.statusId)"
                      [attr.aria-label]="'config.fields.rules.cell' | transloco: { type: typeLabel, step, mode: modeLabel }"
                    >
                      {{ modeLabel }}
                    </button>
                  </td>
                }
              </tr>
            }
          </tbody>
        </table>
      </div>
    } @empty {
      <p class="empty">{{ 'config.fields.rules.empty' | transloco }}</p>
    }
  `,
  styleUrl: './rules-grid.scss',
})
export class RulesGrid {
  readonly types = input.required<readonly IssueType[]>();
  readonly workflows = input.required<readonly Workflow[]>();
  /** The types the field applies to. */
  readonly applicability = input.required<Applicability>();
  /** The cells that are not Visible. */
  readonly rules = model.required<FieldRule[]>();
  readonly disabled = input(false);

  protected readonly grids = computed(() => rulesGrids(this.types(), this.workflows(), this.applicability()));

  /** The creation, then each status. */
  protected columnsOf(statuses: readonly WorkflowStatus[]): { statusId: string | null; labels: Translations | null }[] {
    return [
      { statusId: null, labels: null },
      ...statuses.map((s) => ({ statusId: s.id, labels: s.labelTranslations as Translations })),
    ];
  }

  protected modeOf(typeId: string, statusId: string | null): FieldMode {
    return modeOf(this.rules(), typeId, statusId);
  }

  protected cycle(typeId: string, statusId: string | null): void {
    this.rules.set(cycled(this.rules(), typeId, statusId));
  }
}
