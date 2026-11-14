import { Component, computed, inject, resource, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialog, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfigScreen,
  ErrorState,
  IssueType,
  ScreenKind,
  TranslatableInput,
  TranslatedPipe,
  createScreen,
  getProjectConfig,
  listFields,
  listIssueTypes,
  listScreens,
  listWorkflows,
  translationsForm,
  translationsValue,
} from 'ui';

import { ConfigContext } from './config-page';
import { placeApiErrors } from './form-errors';
import { ScreenEditor } from './screen-editor';
import { SCREEN_KINDS, SCREEN_NAME_MAX } from './screens';

/** A new screen: what it needs. */
export interface ScreenDialogData {
  readonly configId: string;
  readonly types: readonly IssueType[];
  /** The screens already there: a type has one of each kind at most. */
  readonly screens: readonly ConfigScreen[];
}

/** Adds a creation or view screen for a type, or a transition screen. Closes with the screen. */
@Component({
  selector: 'app-screen-dialog',
  imports: [MatButtonModule, MatDialogModule, MatFormFieldModule, MatSelectModule, ReactiveFormsModule, TranslatableInput, TranslatedPipe, TranslocoPipe],
  styleUrl: './config-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ 'config.screens.newTitle' | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <mat-form-field appearance="outline">
          <mat-label>{{ 'config.screens.kind' | transloco }}</mat-label>
          <mat-select formControlName="kind">
            @for (kind of kinds; track kind) {
              <mat-option [value]="kind">{{ 'config.screens.kinds.' + kind | transloco }}</mat-option>
            }
          </mat-select>
        </mat-form-field>
        @if (form.controls.kind.value !== 'transition') {
          <mat-form-field appearance="outline">
            <mat-label>{{ 'config.screens.type' | transloco }}</mat-label>
            <mat-select formControlName="issueTypeId" required>
              @for (issueType of freeTypes(); track issueType.id) {
                <mat-option [value]="issueType.id">{{ issueType.labelTranslations | translated }}</mat-option>
              }
            </mat-select>
            <mat-hint>{{ 'config.screens.typeHint' | transloco }}</mat-hint>
            <mat-error>{{ (form.controls.issueTypeId.getError('api') ?? 'errors.field.required') | transloco }}</mat-error>
          </mat-form-field>
        }
        <ui-translatable-input [group]="form.controls.nameTranslations" [label]="'config.screens.name' | transloco" [maxLength]="nameMax" />
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" class="confirm" [disabled]="saving()">{{ 'config.screens.create' | transloco }}</button>
      </mat-dialog-actions>
    </form>
  `,
})
export class ScreenDialog {
  protected readonly data = inject<ScreenDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<ScreenDialog, ConfigScreen>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly kinds = SCREEN_KINDS;
  protected readonly nameMax = SCREEN_NAME_MAX;
  protected readonly form = new FormGroup({
    kind: new FormControl<ScreenKind>('transition', { nonNullable: true }),
    issueTypeId: new FormControl<string | null>(null),
    nameTranslations: translationsForm({}, { maxLength: SCREEN_NAME_MAX }),
  });
  private readonly kind = signal<ScreenKind>('transition');
  /** The types without a screen of the kind chosen. */
  protected readonly freeTypes = computed(() =>
    this.data.types.filter((t) => !this.data.screens.some((s) => s.kind === this.kind() && s.issueTypeId === t.id)),
  );
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    this.form.controls.kind.valueChanges.subscribe((kind) => {
      this.kind.set(kind);
      this.form.controls.issueTypeId.setValue(null);
      this.form.controls.issueTypeId.setValidators(kind === 'transition' ? null : Validators.required);
      this.form.controls.issueTypeId.updateValueAndValidity();
    });
  }

  protected async save(): Promise<void> {
    for (const control of Object.values(this.form.controls.nameTranslations.controls)) control.updateValueAndValidity();
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const { kind, issueTypeId } = this.form.getRawValue();
    try {
      const saved = await this.api.invoke(createScreen, {
        id: this.data.configId,
        'Idempotency-Key': this.idempotencyKey,
        body: {
          kind,
          issueTypeId: kind === 'transition' ? null : issueTypeId,
          nameTranslations: translationsValue(this.form.controls.nameTranslations),
          fieldIds: [],
        },
      });
      this.dialogRef.close(saved);
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, this.form.controls));
    } finally {
      this.saving.set(false);
    }
  }
}

/**
 * "Screens" tab of a configuration (L04-US06): its creation, view and
 * transition screens, with the type each is for or the transitions that
 * show it; edited in a panel. Changes go to the draft.
 */
@Component({
  selector: 'app-screens-tab',
  imports: [ErrorState, MatButtonModule, MatIconModule, MatProgressBarModule, ScreenEditor, TranslatedPipe, TranslocoPipe],
  styleUrl: './screens-tab.scss',
  templateUrl: './screens-tab.html',
})
export class ScreensTab {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  protected readonly context = inject(ConfigContext);

  protected readonly kinds = SCREEN_KINDS;
  protected readonly configId = computed(() => this.context.config()?.id);
  protected readonly screens = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listScreens, { id: params }),
  });
  protected readonly items = computed(() => (this.screens.hasValue() ? this.screens.value().items : []));
  private readonly fieldList = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listFields, { id: params }),
  });
  protected readonly fields = computed(() => (this.fieldList.hasValue() ? this.fieldList.value().items : []));
  private readonly typeList = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listIssueTypes, { id: params }),
  });
  protected readonly types = computed(() => (this.typeList.hasValue() ? this.typeList.value().items : []));
  private readonly workflowList = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listWorkflows, { id: params }),
  });
  protected readonly workflows = computed(() => (this.workflowList.hasValue() ? this.workflowList.value().items : []));
  private readonly chosen = signal<string | null>(null);
  protected readonly selected = computed(() => this.items().find((s) => s.id === this.chosen()) ?? null);
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);

  protected ofKind(kind: ScreenKind): ConfigScreen[] {
    return this.items().filter((s) => s.kind === kind);
  }

  protected typeOf(screen: ConfigScreen): IssueType | undefined {
    return this.types().find((t) => t.id === screen.issueTypeId);
  }

  protected choose(screen: ConfigScreen): void {
    this.notice.set(null);
    this.chosen.set(screen.id);
  }

  protected close(): void {
    this.chosen.set(null);
  }

  protected add(): void {
    const data: ScreenDialogData = { configId: this.configId()!, types: this.types(), screens: this.items() };
    this.dialog
      .open<ScreenDialog, ScreenDialogData, ConfigScreen>(ScreenDialog, { data, width: 'min(560px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe(async (saved) => {
        if (!saved) return;
        this.chosen.set(saved.id);
        this.notice.set({ ok: true, key: 'config.screens.created', params: { name: saved.name ?? '' } });
        await this.refresh();
      });
  }

  protected async saved(screen: ConfigScreen): Promise<void> {
    this.notice.set({ ok: true, key: 'config.screens.saved', params: { name: screen.name ?? '' } });
    await this.refresh();
  }

  protected async removed(screen: ConfigScreen): Promise<void> {
    this.chosen.set(null);
    this.notice.set({ ok: true, key: 'config.screens.removed', params: { name: screen.name ?? '' } });
    await this.refresh();
  }

  /** The screens again, and the configuration's draft chip. */
  private async refresh(): Promise<void> {
    this.screens.reload();
    const id = this.configId();
    if (id) this.context.config.set(await this.api.invoke(getProjectConfig, { id }));
  }
}
