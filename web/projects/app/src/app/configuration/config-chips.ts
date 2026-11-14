import { Component, computed, input } from '@angular/core';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';
import { LocalNumberPipe, ProjectConfig } from 'ui';

/** The key of a count, from its prefix (`config.shared.`): `….one` for one, `….other` otherwise. */
export function countKey(prefix: string, count: number): string {
  return `${prefix}${count === 1 ? 'one' : 'other'}`;
}

/**
 * How a configuration stands (mockup 11): the default one, the projects
 * sharing it, and its unpublished draft with the number of changes.
 */
@Component({
  selector: 'app-config-chips',
  imports: [LocalNumberPipe, MatIconModule, TranslocoPipe],
  template: `
    @if (config().isDefault) {
      <span class="chip default">
        <mat-icon class="icon" aria-hidden="true">star</mat-icon>{{ 'config.default' | transloco }}
      </span>
    }
    <span class="chip shared">
      <mat-icon class="icon" aria-hidden="true">share</mat-icon
      >{{ sharedKey() | transloco: { count: (config().projectCount | localNumber) } }}
    </span>
    @if (config().draft; as draft) {
      <span class="chip draft">
        <mat-icon class="icon" aria-hidden="true">edit_note</mat-icon
        >{{ draftKey() | transloco: { count: (draft.changeCount | localNumber) } }}
      </span>
    }
  `,
  styleUrl: './config-chips.scss',
})
export class ConfigChips {
  readonly config = input.required<Pick<ProjectConfig, 'isDefault' | 'projectCount' | 'draft'>>();

  protected readonly sharedKey = computed(() =>
    this.config().projectCount === 0 ? 'config.unused' : countKey('config.shared.', this.config().projectCount),
  );
  protected readonly draftKey = computed(() => countKey('config.draft.', this.config().draft?.changeCount ?? 0));
}
