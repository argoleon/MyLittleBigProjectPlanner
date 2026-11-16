import { Component, inject, input, model, resource, signal } from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime } from 'rxjs';
import { Api, IssueRelated, searchIssues } from 'ui';

/**
 * Picks a ticket by its key or part of its summary (L05-US03), among the
 * projects the member sees, or in one project; `accept` leaves out those
 * that cannot be chosen.
 */
@Component({
  selector: 'app-issue-picker',
  imports: [MatAutocompleteModule, MatFormFieldModule, MatInputModule, TranslocoPipe],
  template: `
    <mat-form-field appearance="outline" class="field">
      <mat-label>{{ label() }}</mat-label>
      <input
        #search
        matInput
        [matAutocomplete]="auto"
        [placeholder]="'issue.picker.placeholder' | transloco"
        (input)="query.set(search.value); selected.set(null)"
      />
      <mat-autocomplete #auto="matAutocomplete" [displayWith]="display" (optionSelected)="choose($event)">
        @for (issue of suggestions(); track issue.id) {
          <mat-option [value]="issue">
            <span class="key">{{ issue.key }}</span> {{ issue.summary }}
            @if (!projectKey()) {
              <span class="project"> · {{ issue.project.name }}</span>
            }
          </mat-option>
        } @empty {
          @if (query().trim() && found.hasValue()) {
            <mat-option disabled>{{ 'issue.picker.noMatch' | transloco }}</mat-option>
          }
        }
      </mat-autocomplete>
      <!-- Not a form control: Material shows no mat-error, the hint says it. -->
      @if (error(); as key) {
        <mat-hint class="error" role="alert">{{ key | transloco }}</mat-hint>
      } @else {
        <mat-hint>{{ 'issue.picker.hint' | transloco }}</mat-hint>
      }
    </mat-form-field>
  `,
  styleUrl: './issue-picker.scss',
})
export class IssuePicker {
  private readonly api = inject(Api);

  readonly label = input.required<string>();
  /** Only the tickets of this project. */
  readonly projectKey = input<string | null>(null);
  /** Whether a ticket found can be chosen. */
  readonly accept = input<(issue: IssueRelated) => boolean>(() => true);
  /** Translation key of the error to show, if any. */
  readonly error = input<string | null>(null);
  /** The ticket chosen. */
  readonly selected = model<IssueRelated | null>(null);

  protected readonly query = signal('');
  private readonly debounced = toSignal(toObservable(this.query).pipe(debounceTime(250)), { initialValue: '' });
  protected readonly found = resource({
    params: () => {
      const search = this.debounced().trim();
      return search ? { search, projectKey: this.projectKey() ?? undefined } : undefined;
    },
    loader: ({ params }) => this.api.invoke(searchIssues, params),
  });

  protected readonly display = (issue: IssueRelated | null) => (issue ? `${issue.key} ${issue.summary}` : '');

  protected suggestions(): IssueRelated[] {
    return this.found.hasValue() ? this.found.value().filter((issue) => this.accept()(issue)) : [];
  }

  protected choose(event: MatAutocompleteSelectedEvent): void {
    this.selected.set(event.option.value as IssueRelated);
  }
}
