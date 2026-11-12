import { Component, inject, model, resource, signal } from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatChipsModule } from '@angular/material/chips';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime } from 'rxjs';
import { Api, NodeOwner, listMembers } from 'ui';

/** Owners shown at most in the suggestions. */
const SUGGESTIONS = 8;

/**
 * Owners of a node: a chip per member, and a search among the tenant's
 * members to add one (Material chip grid with an autocomplete).
 */
@Component({
  selector: 'app-owner-picker',
  imports: [MatAutocompleteModule, MatChipsModule, MatFormFieldModule, MatIconModule, TranslocoPipe],
  template: `
    <mat-form-field appearance="outline" class="field">
      <mat-label>{{ 'hierarchy.owners.label' | transloco }}</mat-label>
      <mat-chip-grid #grid [attr.aria-label]="'hierarchy.owners.label' | transloco">
        @for (owner of owners(); track owner.accountId) {
          <mat-chip-row (removed)="remove(owner)">
            {{ owner.displayName }}
            <button matChipRemove type="button" [attr.aria-label]="'hierarchy.owners.removeOf' | transloco: { name: owner.displayName }">
              <mat-icon aria-hidden="true">cancel</mat-icon>
            </button>
          </mat-chip-row>
        }
        <input
          #search
          [placeholder]="'hierarchy.owners.add' | transloco"
          [matChipInputFor]="grid"
          [matAutocomplete]="auto"
          (input)="query.set(search.value)"
        />
      </mat-chip-grid>
      <mat-autocomplete #auto="matAutocomplete" (optionSelected)="add($event); search.value = ''; query.set('')">
        @for (member of suggestions(); track member.accountId) {
          <mat-option [value]="member">{{ member.displayName }} · {{ member.email }}</mat-option>
        } @empty {
          @if (query().trim()) {
            <mat-option disabled>{{ 'hierarchy.owners.noMatch' | transloco }}</mat-option>
          }
        }
      </mat-autocomplete>
    </mat-form-field>
  `,
  styleUrl: './owner-picker.scss',
})
export class OwnerPicker {
  private readonly api = inject(Api);

  /** The owners, in the order they were added. */
  readonly owners = model<readonly NodeOwner[]>([]);

  protected readonly query = signal('');
  private readonly debounced = toSignal(toObservable(this.query).pipe(debounceTime(250)), { initialValue: '' });
  private readonly found = resource({
    params: () => this.debounced().trim() || undefined,
    loader: ({ params }) => this.api.invoke(listMembers, { search: params, limit: SUGGESTIONS }),
  });

  /** Members matching the search, the owners already chosen left out. */
  protected suggestions() {
    if (!this.found.hasValue()) return [];
    const chosen = new Set(this.owners().map((o) => o.accountId));
    return this.found.value().items.filter((m) => !chosen.has(m.accountId));
  }

  protected add(event: MatAutocompleteSelectedEvent): void {
    const member = event.option.value as { accountId: string; displayName: string };
    this.owners.update((owners) => [...owners, { accountId: member.accountId, displayName: member.displayName }]);
  }

  protected remove(owner: NodeOwner): void {
    this.owners.update((owners) => owners.filter((o) => o.accountId !== owner.accountId));
  }
}
