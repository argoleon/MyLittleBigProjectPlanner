import { Component, ElementRef, forwardRef, inject, input, resource, signal, viewChild } from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import { ControlValueAccessor, NG_VALUE_ACCESSOR } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatChipsModule } from '@angular/material/chips';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime } from 'rxjs';
import { Api, IssuePerson, listProjectPeople } from 'ui';

/**
 * Members (or groups) of the organization a ticket of a project names: a
 * chip each, and a search to add one. A form control holding the people
 * chosen; one at most unless `multiple`.
 */
@Component({
  selector: 'app-people-picker',
  imports: [MatAutocompleteModule, MatChipsModule, MatFormFieldModule, MatIconModule, TranslocoPipe],
  providers: [{ provide: NG_VALUE_ACCESSOR, useExisting: forwardRef(() => PeoplePicker), multi: true }],
  template: `
    <mat-form-field appearance="outline" class="field">
      <mat-label>{{ label() }}</mat-label>
      <mat-chip-grid #grid [disabled]="disabled()" [attr.aria-label]="label()" [required]="required()">
        @for (person of people(); track person.id) {
          <mat-chip-row (removed)="remove(person)">
            {{ person.name }}
            <button matChipRemove type="button" [attr.aria-label]="'issue.people.remove' | transloco: { name: person.name }">
              <mat-icon aria-hidden="true">cancel</mat-icon>
            </button>
          </mat-chip-row>
        }
      </mat-chip-grid>
      <!-- Out of the grid, which holds only its rows (axe); linked by matChipInputFor. -->
      @if (!disabled() && !full()) {
        <input
          #search
          [placeholder]="(kind() === 'group' ? 'issue.people.searchGroup' : 'issue.people.search') | transloco"
          [matChipInputFor]="grid"
          [matAutocomplete]="auto"
          (input)="query.set(search.value)"
          (focus)="active.set(true)"
          (blur)="onTouched()"
        />
      }
      <mat-autocomplete #auto="matAutocomplete" (optionSelected)="add($event)">
        @for (person of suggestions(); track person.id) {
          <mat-option [value]="person">{{ person.name }}</mat-option>
        } @empty {
          @if (query().trim()) {
            <mat-option disabled>{{ 'issue.people.noMatch' | transloco }}</mat-option>
          }
        }
      </mat-autocomplete>
      @if (error(); as key) {
        <mat-error>{{ key | transloco }}</mat-error>
      }
    </mat-form-field>
  `,
  styleUrl: './people-picker.scss',
})
export class PeoplePicker implements ControlValueAccessor {
  private readonly api = inject(Api);

  readonly projectKey = input.required<string>();
  readonly label = input.required<string>();
  readonly kind = input<'member' | 'group'>('member');
  readonly multiple = input(false);
  readonly required = input(false);
  /** Translation key of the error to show, if any. */
  readonly error = input<string | null>(null);

  private readonly search = viewChild<ElementRef<HTMLInputElement>>('search');
  protected readonly people = signal<readonly IssuePerson[]>([]);
  protected readonly disabled = signal(false);
  protected readonly query = signal('');
  /** The search is used: suggestions load from then on. */
  protected readonly active = signal(false);
  private readonly debounced = toSignal(toObservable(this.query).pipe(debounceTime(250)), { initialValue: '' });
  private readonly found = resource({
    params: () => (this.active() ? { key: this.projectKey(), kind: this.kind(), search: this.debounced().trim() } : undefined),
    loader: ({ params }) => this.api.invoke(listProjectPeople, { key: params.key, kind: params.kind, search: params.search || undefined }),
  });
  private onChange: (value: readonly IssuePerson[]) => void = () => undefined;
  protected onTouched: () => void = () => undefined;

  /** A single choice is made: no search until it is removed. */
  protected full(): boolean {
    return !this.multiple() && this.people().length > 0;
  }

  /** Matches of the search, those chosen left out. */
  protected suggestions(): IssuePerson[] {
    if (!this.found.hasValue()) return [];
    const chosen = new Set(this.people().map((p) => p.id));
    return this.found.value().filter((p) => !chosen.has(p.id));
  }

  writeValue(value: readonly IssuePerson[] | null): void {
    this.people.set(value ?? []);
  }

  registerOnChange(fn: (value: readonly IssuePerson[]) => void): void {
    this.onChange = fn;
  }

  registerOnTouched(fn: () => void): void {
    this.onTouched = fn;
  }

  setDisabledState(disabled: boolean): void {
    this.disabled.set(disabled);
  }

  protected add(event: MatAutocompleteSelectedEvent): void {
    const person = event.option.value as IssuePerson;
    this.people.update((people) => (this.multiple() ? [...people, person] : [person]));
    this.onChange(this.people());
    const search = this.search()?.nativeElement;
    if (search) search.value = '';
    this.query.set('');
  }

  protected remove(person: IssuePerson): void {
    this.people.update((people) => people.filter((p) => p.id !== person.id));
    this.onChange(this.people());
    this.onTouched();
  }
}
