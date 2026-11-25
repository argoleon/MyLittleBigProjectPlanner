import { Component, computed, inject, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime, map } from 'rxjs';
import { Api, Resource, Team, addTeamMember, errorTranslationKey, isApiProblem, listResources } from 'ui';

/** The team a resource is added to. */
export interface AddMemberData {
  readonly team: Team;
}

/** Resources offered at once in the search. */
const CHOICES = 20;

/**
 * Adds a resource to a team (L07-US02), found by name or job title.
 * Closes with the resource added.
 */
@Component({
  selector: 'app-add-member-dialog',
  imports: [
    MatAutocompleteModule,
    MatButtonModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: '../admin/provider-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ 'teams.addDialog.title' | transloco: { name: data.team.name } }}</h2>
    <form (submit)="submit($event)" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as errorKey) {
          <p class="message error" role="alert">{{ errorKey | transloco }}</p>
        }
        <mat-form-field appearance="outline">
          <mat-label>{{ 'teams.addDialog.resource' | transloco }}</mat-label>
          <mat-icon matPrefix aria-hidden="true">person_search</mat-icon>
          <input
            matInput
            type="text"
            class="resource-input"
            [formControl]="input"
            [matAutocomplete]="resources"
            (input)="chosen.set(null)"
          />
          <mat-hint>{{ 'teams.addDialog.hint' | transloco }}</mat-hint>
          <mat-autocomplete #resources="matAutocomplete" [displayWith]="nameOf" (optionSelected)="choose($event)">
            @for (choice of choices(); track choice.id) {
              <mat-option [value]="choice" [disabled]="inTeam(choice)">
                <span>{{ choice.displayName }}</span>
                @if (inTeam(choice)) {
                  <span class="choice-hint"> · {{ 'teams.addDialog.inTeam' | transloco }}</span>
                } @else if (choice.jobTitle) {
                  <span class="choice-hint"> · {{ choice.jobTitle }}</span>
                }
              </mat-option>
            }
          </mat-autocomplete>
        </mat-form-field>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" [disabled]="saving() || !chosen()">{{ 'teams.addDialog.add' | transloco }}</button>
      </mat-dialog-actions>
    </form>
  `,
})
export class AddMemberDialog {
  protected readonly data = inject<AddMemberData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<AddMemberDialog, Resource>>(MatDialogRef);

  protected readonly input = new FormControl<string | Resource>('', { nonNullable: true });
  private readonly search = toSignal(
    this.input.valueChanges.pipe(
      debounceTime(250),
      map((value) => (typeof value === 'string' ? value.trim() : '')),
    ),
    { initialValue: '' },
  );
  private readonly found = resource({
    params: () => ({ search: this.search() }),
    loader: ({ params }) => this.api.invoke(listResources, { search: params.search || undefined, active: true, limit: CHOICES }),
  });
  protected readonly choices = computed<readonly Resource[]>(() => (this.found.hasValue() ? this.found.value().items : []));
  protected readonly chosen = signal<Resource | null>(null);
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected readonly nameOf = (value: string | Resource | null): string =>
    typeof value === 'string' ? value : (value?.displayName ?? '');

  protected inTeam(resource: Resource): boolean {
    return resource.teams.some((team) => team.id === this.data.team.id);
  }

  protected choose(event: MatAutocompleteSelectedEvent): void {
    this.chosen.set(event.option.value as Resource);
  }

  /** A plain form: its submission stays in the page. */
  protected submit(event: Event): void {
    event.preventDefault();
    void this.add();
  }

  private async add(): Promise<void> {
    const chosen = this.chosen();
    if (!chosen) return;
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      await this.api.invoke(addTeamMember, { id: this.data.team.id, resourceId: chosen.id });
      this.dialogRef.close(chosen);
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.saving.set(false);
    }
  }
}
