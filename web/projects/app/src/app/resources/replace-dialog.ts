import { Component, computed, inject, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatButtonModule } from '@angular/material/button';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import { debounceTime, filter, map } from 'rxjs';
import {
  Api,
  LocalDatePipe,
  LocaleService,
  OverloadDay,
  Resource,
  ResourceReplacement,
  ResourceReplacementTicket,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  listResources,
  previewResourceReplacement,
  replaceResource,
  toLocalDate,
} from 'ui';

import { overloadDaysText } from '../issues/overload-text';

/** The generic resource to replace. */
export interface ReplaceDialogData {
  readonly generic: Resource;
}

/** What the dialog closes with: the replacement made, and the person. */
export interface ReplaceDialogResult {
  readonly replacement: ResourceReplacement;
  readonly person: Resource;
}

/** People offered at once in the search. */
const CHOICES = 20;

/** The tickets in words: their keys, `hidden` for those the member does not see. */
export function ticketsText(tickets: readonly ResourceReplacementTicket[], hidden: string): string {
  return tickets.map((t) => t.issueKey ?? hidden).join(', ');
}

/** The overload to tell, from the preview or a refusal; `null` when none. */
export interface OverloadNotice {
  readonly days: readonly OverloadDay[];
  readonly blocked: boolean;
  readonly forceable: boolean;
}

/**
 * Replaces a generic resource with a person (L08-US08): the person, found
 * by name or job title among the active people, and whether to deactivate
 * the generic resource afterwards (checked by default). Once a person is
 * chosen, a preview tells the assignments that would move from today on —
 * joining the person's assignment of a ticket when there is one —, the
 * tickets where segments would take a same day (which refuse it), those
 * of archived projects left behind, and the person's days it would
 * overload. An overload in blocking mode keeps it from being made unless
 * the member may force it ("Force overload", traced). Closes with the
 * replacement made.
 */
@Component({
  selector: 'app-replace-dialog',
  imports: [
    LocalDatePipe,
    MatAutocompleteModule,
    MatButtonModule,
    MatCheckboxModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatProgressBarModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  templateUrl: './replace-dialog.html',
  styleUrl: './replace-dialog.scss',
})
export class ReplaceDialog {
  protected readonly data = inject<ReplaceDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<ReplaceDialog, ReplaceDialogResult>>(MatDialogRef);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);

  protected readonly today = toLocalDate(new Date());
  protected readonly personInput = new FormControl<string | Resource>('', { nonNullable: true });
  protected readonly deactivate = new FormControl(true, { nonNullable: true });
  private readonly search = toSignal(
    this.personInput.valueChanges.pipe(
      // Choosing a person puts them in the field: only typed text searches.
      filter((value): value is string => typeof value === 'string'),
      debounceTime(250),
      map((value) => value.trim()),
    ),
    { initialValue: '' },
  );
  private readonly found = resource({
    params: () => ({ search: this.search() }),
    loader: ({ params }) =>
      this.api.invoke(listResources, { search: params.search || undefined, kind: 'person', active: true, limit: CHOICES }),
  });
  protected readonly choices = computed<readonly Resource[]>(() => (this.found.hasValue() ? this.found.value().items : []));
  protected readonly chosen = signal<Resource | null>(null);
  protected readonly preview = resource({
    params: () => this.chosen()?.id,
    loader: ({ params }) => this.api.invoke(previewResourceReplacement, { id: this.data.generic.id, body: { personId: params } }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  /** The last replacement refused for overload; choosing again clears it. */
  private readonly refusal = signal<OverloadNotice | null>(null);

  protected readonly told = computed<ResourceReplacement | null>(() => (this.chosen() && this.preview.hasValue() ? this.preview.value() : null));
  protected readonly hidden = computed(() => this.transloco.translate('resources.replace.hidden'));
  protected readonly overload = computed<OverloadNotice | null>(() => {
    const refusal = this.refusal();
    if (refusal) return refusal;
    const told = this.told();
    if (!told || told.overloads.length === 0) return null;
    return { days: told.overloads, blocked: told.mode === 'blocking', forceable: told.forceable };
  });
  protected readonly overloadText = computed(() => {
    const overload = this.overload();
    if (!overload) return '';
    return overloadDaysText(overload.days, this.locale.formattingLocale(), (key, params) => this.transloco.translate(key, params));
  });
  /** The replacement cannot be made as it is: overlaps, or an overload refused. */
  protected readonly blocked = computed(() => (this.told()?.overlaps.length ?? 0) > 0 || (this.overload()?.blocked ?? false));

  protected readonly nameOf = (value: string | Resource | null): string =>
    typeof value === 'string' ? value : (value?.displayName ?? '');

  protected readonly ticketsText = ticketsText;

  protected choose(event: MatAutocompleteSelectedEvent): void {
    this.chosen.set(event.option.value as Resource);
    this.personInput.setErrors(null);
    this.refusal.set(null);
    this.errorKey.set(null);
  }

  /** Typing again drops the person chosen. */
  protected typed(): void {
    this.chosen.set(null);
    this.refusal.set(null);
  }

  /** A plain form: its submission stays in the page. */
  protected submit(event: Event): void {
    event.preventDefault();
    void this.replace(false);
  }

  /** Replaces beyond capacity: members with "Force overload" only; traced. */
  protected force(): void {
    void this.replace(true);
  }

  private async replace(force: boolean): Promise<void> {
    const person = this.chosen();
    if (!person) {
      this.personInput.setErrors({ api: 'errors.field.required' });
      this.personInput.markAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const replacement = await this.api.invoke(replaceResource, {
        id: this.data.generic.id,
        body: { personId: person.id, deactivate: this.deactivate.value, force },
      });
      this.dialogRef.close({ replacement, person });
    } catch (error) {
      this.errorKey.set(this.placeErrors(error));
    } finally {
      this.saving.set(false);
    }
  }

  /** Puts the person's errors on its field; the key to show above the form, if any. */
  private placeErrors(error: unknown): string | null {
    if (!isApiProblem(error)) return errorTranslationKey('unknown');
    if (error.overload) {
      this.refusal.set({ days: error.overload.days, blocked: true, forceable: error.overload.forceable });
      return null;
    }
    const onPerson = error.fieldErrors.find((e) => e.field === 'personId');
    if (onPerson) {
      this.personInput.setErrors({ api: fieldErrorTranslationKey(onPerson.code) });
      this.personInput.markAsTouched();
      return null;
    }
    // Overlaps: the preview tells them again.
    if (error.code === 'assignment.segments_overlap') this.preview.reload();
    return error.translationKey;
  }
}
