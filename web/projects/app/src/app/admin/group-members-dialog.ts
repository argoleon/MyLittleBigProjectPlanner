import { Component, computed, inject, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime } from 'rxjs';
import {
  Api,
  ErrorState,
  Group,
  Member,
  addGroupMember,
  errorTranslationKey,
  getGroup,
  isApiProblem,
  listMembers,
  removeGroupMember,
} from 'ui';

/**
 * The members of a group: add one found by name or email, remove one.
 * Every change applies at once.
 */
@Component({
  selector: 'app-group-members-dialog',
  imports: [
    ErrorState,
    MatAutocompleteModule,
    MatButtonModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatProgressBarModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrls: ['./provider-dialog.scss', './group-members-dialog.scss'],
  template: `
    <h2 mat-dialog-title>{{ 'admin.groups.membersOf' | transloco: { name: group.name } }}</h2>
    <mat-dialog-content class="fields">
      @if (errorKey(); as key) {
        <p class="message error" role="alert">{{ key | transloco }}</p>
      }
      <mat-form-field appearance="outline">
        <mat-label>{{ 'admin.groups.addMember' | transloco }}</mat-label>
        <mat-icon matPrefix aria-hidden="true">person_add</mat-icon>
        <input
          matInput
          type="search"
          [formControl]="searchControl"
          [matAutocomplete]="picker"
          [placeholder]="'admin.members.searchPlaceholder' | transloco"
        />
        <mat-autocomplete #picker="matAutocomplete" (optionSelected)="add($event)">
          @for (candidate of candidates(); track candidate.accountId) {
            <mat-option [value]="candidate">
              <span class="option-name">{{ candidate.displayName }}</span>
              <span class="option-email">{{ candidate.email }}</span>
            </mat-option>
          }
        </mat-autocomplete>
      </mat-form-field>

      @if (detail.isLoading() && !detail.hasValue()) {
        <mat-progress-bar mode="indeterminate" [attr.aria-label]="'state.loading' | transloco" />
      } @else if (detail.error()) {
        <ui-error-state [error]="detail.error()" [headingLevel]="3" (retry)="detail.reload()" />
      } @else if (detail.value()?.members?.length === 0) {
        <p class="choice-hint">{{ 'admin.groups.noMember' | transloco }}</p>
      } @else {
        <ul class="members" [attr.aria-label]="'admin.groups.membersOf' | transloco: { name: group.name }">
          @for (member of detail.value()?.members; track member.accountId) {
            <li class="member">
              <span class="member-text">
                <span class="option-name">{{ member.displayName }}</span>
                <span class="option-email">{{ member.email }}</span>
              </span>
              <button
                matIconButton
                type="button"
                (click)="remove(member.accountId)"
                [attr.aria-label]="'admin.groups.removeMemberOf' | transloco: { name: member.displayName }"
              >
                <mat-icon aria-hidden="true">close</mat-icon>
              </button>
            </li>
          }
        </ul>
      }
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton="filled" type="button" mat-dialog-close>{{ 'dialog.close' | transloco }}</button>
    </mat-dialog-actions>
  `,
})
export class GroupMembersDialog {
  protected readonly group = inject<Group>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);

  protected readonly detail = resource({ loader: () => this.api.invoke(getGroup, { id: this.group.id }) });
  protected readonly searchControl = new FormControl<string | Member>('', { nonNullable: true });
  private readonly search = toSignal(this.searchControl.valueChanges.pipe(debounceTime(250)), { initialValue: '' });
  private readonly found = resource({
    params: () => {
      const search = this.search();
      return typeof search === 'string' && search.trim() ? search.trim() : undefined;
    },
    loader: async ({ params }) => (await this.api.invoke(listMembers, { search: params, limit: 20 })).items,
  });
  /** Members found, without those already in the group. */
  protected readonly candidates = computed(() => {
    const inGroup = new Set((this.detail.hasValue() ? this.detail.value().members : []).map((m) => m.accountId));
    return (this.found.hasValue() ? this.found.value() : []).filter((m) => !inGroup.has(m.accountId));
  });
  protected readonly errorKey = signal<string | null>(null);

  protected async add(event: MatAutocompleteSelectedEvent): Promise<void> {
    const member = event.option.value as Member;
    this.searchControl.setValue('');
    await this.change(() => this.api.invoke(addGroupMember, { id: this.group.id, accountId: member.accountId }));
  }

  protected async remove(accountId: string): Promise<void> {
    await this.change(() => this.api.invoke(removeGroupMember, { id: this.group.id, accountId }));
  }

  private async change(call: () => Promise<unknown>): Promise<void> {
    this.errorKey.set(null);
    try {
      await call();
      this.detail.reload();
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    }
  }
}
