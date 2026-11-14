import { CdkDrag, CdkDragDrop, CdkDragHandle, CdkDropList, CdkDropListGroup } from '@angular/cdk/drag-drop';
import { Component, computed, effect, inject, input, output, signal, untracked } from '@angular/core';
import { FormGroup, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfigField,
  ConfigScreen,
  ConfirmDialog,
  ConfirmDialogData,
  IssueType,
  TranslatableInput,
  TranslatedPipe,
  Workflow,
  deleteScreen,
  errorTranslationKey,
  isApiProblem,
  translationsForm,
  translationsValue,
  updateScreen,
} from 'ui';

import { placeApiErrors } from './form-errors';
import { SCREEN_NAME_MAX, available, missingRequired, moved, preview, stepsOf, typesOf, withField, withoutField } from './screens';

/**
 * The panel of a screen (L04-US06): its name, the fields on it in their
 * order (dragged from the available ones, or added, moved and removed with
 * buttons), the required fields it lacks, and a preview for a type and a
 * step where the rules win: hidden fields shown disabled, required ones
 * marked, missing required ones added at the end. Saves to the draft.
 */
@Component({
  selector: 'app-screen-editor',
  imports: [
    CdkDrag,
    CdkDragHandle,
    CdkDropList,
    CdkDropListGroup,
    MatButtonModule,
    MatFormFieldModule,
    MatIconModule,
    MatSelectModule,
    ReactiveFormsModule,
    TranslatableInput,
    TranslatedPipe,
    TranslocoPipe,
  ],
  styleUrl: './screen-editor.scss',
  templateUrl: './screen-editor.html',
})
export class ScreenEditor {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  readonly configId = input.required<string>();
  readonly screen = input.required<ConfigScreen>();
  readonly fields = input.required<readonly ConfigField[]>();
  readonly types = input.required<readonly IssueType[]>();
  readonly workflows = input.required<readonly Workflow[]>();
  readonly saved = output<ConfigScreen>();
  readonly removed = output<ConfigScreen>();
  readonly closed = output<void>();

  protected readonly nameMax = SCREEN_NAME_MAX;
  protected readonly form = new FormGroup({ name: translationsForm({}, { maxLength: SCREEN_NAME_MAX }) });
  /** The fields on the screen, in their order. */
  protected readonly fieldIds = signal<string[]>([]);
  protected readonly onScreen = computed(() =>
    this.fieldIds()
      .map((id) => this.fields().find((f) => f.id === id))
      .filter((f): f is ConfigField => f !== undefined),
  );
  protected readonly availableFields = computed(() => available(this.fieldIds(), this.fields()));
  protected readonly missing = computed(() =>
    missingRequired(this.screen(), this.fieldIds(), this.fields(), this.types(), this.workflows()),
  );
  protected readonly previewTypes = computed(() => typesOf(this.screen(), this.types()));
  protected readonly previewType = signal<string | null>(null);
  protected readonly previewStatus = signal<string | null>(null);
  /** The statuses the preview may pick, for the type chosen. */
  protected readonly previewStatuses = computed(() => {
    const type = this.types().find((t) => t.id === this.previewType());
    const ids = type ? stepsOf(this.screen().kind, type, this.workflows()) : [];
    const statuses = this.workflows().flatMap((w) => w.statuses);
    return ids.map((id) => statuses.find((s) => s.id === id)).filter((s) => s !== undefined);
  });
  protected readonly lines = computed(() => {
    const type = this.previewType();
    if (!type) return [];
    const status = this.screen().kind === 'creation' ? null : this.previewStatus();
    if (this.screen().kind !== 'creation' && !status) return [];
    return preview(this.fieldIds(), this.fields(), type, status);
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  protected readonly errorDetail = signal<string | null>(null);

  constructor() {
    // Another screen to edit: the panel starts again from it.
    effect(() => {
      const screen = this.screen();
      untracked(() => this.reset(screen));
    });
  }

  private reset(screen: ConfigScreen): void {
    this.errorKey.set(null);
    this.errorDetail.set(null);
    this.form.setControl('name', translationsForm(screen.nameTranslations, { maxLength: SCREEN_NAME_MAX }));
    this.fieldIds.set([...screen.fieldIds]);
    const type = typesOf(screen, this.types())[0];
    this.previewType.set(type?.id ?? null);
    this.previewStatus.set(type ? (stepsOf(screen.kind, type, this.workflows())[0] ?? null) : null);
  }

  protected chooseType(id: string): void {
    this.previewType.set(id);
    const type = this.types().find((t) => t.id === id);
    this.previewStatus.set(type ? (stepsOf(this.screen().kind, type, this.workflows())[0] ?? null) : null);
  }

  protected add(field: ConfigField): void {
    this.fieldIds.set(withField(this.fieldIds(), field.id));
  }

  protected remove(field: ConfigField): void {
    this.fieldIds.set(withoutField(this.fieldIds(), field.id));
  }

  protected move(from: number, to: number): void {
    this.fieldIds.set(moved(this.fieldIds(), from, to));
  }

  /** Dropped on the screen: moved within it, or added from the available fields. */
  protected dropOnScreen(event: CdkDragDrop<ConfigField[], ConfigField[], ConfigField>): void {
    if (event.previousContainer === event.container) this.move(event.previousIndex, event.currentIndex);
    else this.fieldIds.set(withField(this.fieldIds(), event.item.data.id, event.currentIndex));
  }

  /** Dropped back on the available fields: taken off the screen. */
  protected dropOnAvailable(event: CdkDragDrop<ConfigField[], ConfigField[], ConfigField>): void {
    if (event.previousContainer !== event.container) this.remove(event.item.data);
  }

  protected async save(): Promise<void> {
    for (const control of Object.values(this.form.controls.name.controls)) control.updateValueAndValidity();
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    this.errorDetail.set(null);
    const screen = this.screen();
    try {
      const saved = await this.api.invoke(updateScreen, {
        id: this.configId(),
        screenId: screen.id,
        'If-Match': `"${screen.version}"`,
        body: { nameTranslations: translationsValue(this.form.controls.name), fieldIds: this.fieldIds() },
      });
      this.saved.emit(saved);
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, { nameTranslations: this.form.controls.name }));
    } finally {
      this.saving.set(false);
    }
  }

  protected confirmRemove(screen: ConfigScreen): void {
    const data: ConfirmDialogData = {
      titleKey: 'config.screens.remove.title',
      messageKey: 'config.screens.remove.message',
      confirmKey: 'config.screens.remove.confirm',
      params: { name: screen.name ?? '' },
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        this.errorKey.set(null);
        this.errorDetail.set(null);
        try {
          await this.api.invoke(deleteScreen, { id: this.configId(), screenId: screen.id, 'If-Match': `"${screen.version}"` });
          this.removed.emit(screen);
        } catch (error) {
          this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
          // The transitions that show it.
          this.errorDetail.set(isApiProblem(error) && error.code === 'screen.in_use' ? (error.detail ?? null) : null);
        }
      });
  }
}
