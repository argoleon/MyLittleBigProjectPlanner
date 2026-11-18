import { Component, computed, inject, resource, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatChipsModule } from '@angular/material/chips';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatRadioModule } from '@angular/material/radio';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  SavedFilter,
  SavedFilterGroup,
  createSavedFilter,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  listSavedFilterShareGroups,
  updateSavedFilter,
} from 'ui';

/** The icons a filter may show, as the API accepts them; the first by default. */
export const FILTER_ICONS = [
  'filter_alt',
  'person',
  'bug_report',
  'schedule',
  'shield',
  'flag',
  'star',
  'support_agent',
  'bolt',
  'label',
] as const;

/** Who a filter is for. */
export type Sharing = 'private' | 'groups' | 'all';

/** What the dialog saves: `query`, as a new filter or into `filter` (the member's). */
export interface SaveFilterData {
  readonly query: string;
  readonly filter?: SavedFilter;
}

/** The sharing of `filter`, as the dialog shows it. */
export function sharingOf(filter: SavedFilter | undefined): Sharing {
  if (!filter) return 'private';
  if (filter.sharing.all) return 'all';
  return filter.sharing.groups.length > 0 ? 'groups' : 'private';
}

/**
 * Saves a filter (L06-US04): its name, its icon, who it is for: the member
 * only, groups, or the whole organization. With `filter`, changes that
 * filter, or saves the query as a new one. Closes with the filter saved.
 */
@Component({
  selector: 'app-save-filter-dialog',
  imports: [
    MatAutocompleteModule,
    MatButtonModule,
    MatButtonToggleModule,
    MatChipsModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatRadioModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  templateUrl: './save-filter-dialog.html',
  styleUrl: './save-filter-dialog.scss',
})
export class SaveFilterDialog {
  protected readonly data = inject<SaveFilterData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<SaveFilterDialog, SavedFilter>>(MatDialogRef);

  protected readonly icons = FILTER_ICONS;
  protected readonly form = inject(FormBuilder).nonNullable.group({
    name: [this.data.filter?.name ?? '', Validators.required],
    icon: [this.data.filter?.icon ?? FILTER_ICONS[0]],
    sharing: [sharingOf(this.data.filter)],
  });
  /** The groups chosen, when shared with groups. */
  protected readonly groups = signal<readonly SavedFilterGroup[]>(this.data.filter?.sharing.groups ?? []);
  protected readonly sharing = signal<Sharing>(sharingOf(this.data.filter));
  protected readonly groupSearch = signal('');
  protected readonly groupError = signal<string | null>(null);
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  private readonly idempotencyKey = crypto.randomUUID();

  private readonly found = resource({
    params: () => (this.sharing() === 'groups' ? this.groupSearch().trim() : undefined),
    loader: ({ params }) => this.api.invoke(listSavedFilterShareGroups, { search: params || undefined, limit: 10 }),
  });
  /** The groups found, those already chosen left out. */
  protected readonly suggestions = computed(() => {
    const chosen = new Set(this.groups().map((g) => g.id));
    return this.found.hasValue() ? this.found.value().filter((g) => !chosen.has(g.id)) : [];
  });

  constructor() {
    this.form.controls.sharing.valueChanges.subscribe((sharing) => {
      this.sharing.set(sharing);
      this.groupError.set(null);
    });
  }

  protected addGroup(event: MatAutocompleteSelectedEvent, input: HTMLInputElement): void {
    const group = event.option.value as SavedFilterGroup;
    this.groups.update((groups) => [...groups, group]);
    this.groupError.set(null);
    input.value = '';
    this.groupSearch.set('');
  }

  protected removeGroup(group: SavedFilterGroup): void {
    this.groups.update((groups) => groups.filter((g) => g.id !== group.id));
  }

  /** Saves into the member's filter, or (`asNew`) as a new one. */
  protected async save(asNew = false): Promise<void> {
    const { name, icon, sharing } = this.form.getRawValue();
    if (sharing === 'groups' && this.groups().length === 0) {
      this.groupError.set('search.filters.dialog.groupsRequired');
    }
    if (this.form.invalid || this.groupError()) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const body = {
      name: name.trim(),
      icon,
      query: this.data.query,
      sharing: { all: sharing === 'all', groupIds: sharing === 'groups' ? this.groups().map((g) => g.id) : [] },
    };
    const filter = this.data.filter;
    try {
      const saved =
        filter && !asNew
          ? await this.api.invoke(updateSavedFilter, { id: filter.id, 'If-Match': `"${filter.version}"`, body })
          : await this.api.invoke(createSavedFilter, { 'Idempotency-Key': asNew ? crypto.randomUUID() : this.idempotencyKey, body });
      this.dialogRef.close(saved);
    } catch (error) {
      const nameError = isApiProblem(error) ? error.fieldErrors.find((e) => e.field === 'name') : undefined;
      const groupsError = isApiProblem(error) ? error.fieldErrors.find((e) => e.field === 'sharing.groupIds') : undefined;
      if (nameError) {
        this.form.controls.name.setErrors({ api: fieldErrorTranslationKey(nameError.code) });
        this.form.controls.name.markAsTouched();
      } else if (groupsError) {
        this.groupError.set('search.filters.dialog.groupsGone');
      } else if (isApiProblem(error) && error.code === 'query.invalid') {
        this.errorKey.set('search.filters.dialog.queryInvalid');
      } else {
        this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      }
    } finally {
      this.saving.set(false);
    }
  }
}
