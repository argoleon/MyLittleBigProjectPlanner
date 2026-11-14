import { Component, computed, inject, resource, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatButtonModule } from '@angular/material/button';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { MatChipsModule } from '@angular/material/chips';
import { MAT_DIALOG_DATA, MatDialog, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { MatTableModule } from '@angular/material/table';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  Api,
  ErrorState,
  GrantCandidate,
  HolderKind,
  LocaleService,
  NamedRef,
  NotificationRecipientKind,
  NotificationRule,
  NotificationRuleList,
  TranslatedPipe,
  getProjectConfig,
  listNotificationRules,
  listTransitionCandidates,
  listTransitionRoles,
  resolveLabel,
  updateNotificationRule,
} from 'ui';

import { ConfigContext } from './config-page';
import { placeApiErrors } from './form-errors';
import { Recipient, kindsFor, recipientsOf, withRef, withoutRef } from './notifications';

/** A rule to change, with the names of what it names. */
export interface NotificationDialogData {
  readonly configId: string;
  readonly rule: NotificationRule;
  readonly references: NotificationRuleList['references'] | null;
}

/** Changes whom an event notifies. Closes with the rules saved. */
@Component({
  selector: 'app-notification-dialog',
  imports: [
    MatAutocompleteModule,
    MatButtonModule,
    MatCheckboxModule,
    MatChipsModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatSelectModule,
    ReactiveFormsModule,
    TranslatedPipe,
    TranslocoPipe,
  ],
  styleUrl: './config-dialog.scss',
  templateUrl: './notification-dialog.html',
})
export class NotificationDialog {
  protected readonly data = inject<NotificationDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<NotificationDialog, NotificationRuleList>>(MatDialogRef);

  protected readonly kinds = kindsFor(this.data.rule.event);
  protected readonly form = new FormGroup({
    kinds: new FormGroup(
      Object.fromEntries(
        this.kinds.map((kind) => [kind, new FormControl((this.data.rule.recipientKinds ?? []).includes(kind), { nonNullable: true })]),
      ) as Record<NotificationRecipientKind, FormControl<boolean>>,
    ),
    roleIds: new FormControl<string[]>([...(this.data.rule.roleIds ?? [])], { nonNullable: true }),
  });
  protected readonly groups = signal<NamedRef[]>(
    (this.data.rule.groupIds ?? []).map((id) => this.data.references?.groups.find((g) => g.id === id) ?? { id, name: '' }),
  );
  protected readonly members = signal<NamedRef[]>(
    (this.data.rule.accountIds ?? []).map((id) => this.data.references?.members.find((m) => m.id === id) ?? { id, name: '' }),
  );
  private readonly roleList = resource({ loader: () => this.api.invoke(listTransitionRoles, {}) });
  protected readonly roles = computed(() => (this.roleList.hasValue() ? this.roleList.value() : []));
  protected readonly groupOptions = signal<NamedRef[]>([]);
  protected readonly memberOptions = signal<GrantCandidate[]>([]);
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  private searches = 0;

  /** The groups or members whose name holds `text`. */
  protected async search(text: string, kind: HolderKind): Promise<void> {
    const search = ++this.searches;
    const found = await this.api.invoke(listTransitionCandidates, { search: text.trim() || undefined, limit: 10 });
    if (search !== this.searches) return;
    const matching = found.filter((c) => c.kind === kind);
    if (kind === 'group') this.groupOptions.set(matching.map((c) => ({ id: c.id, name: c.name })));
    else this.memberOptions.set(matching);
  }

  protected addGroup(event: MatAutocompleteSelectedEvent, input: HTMLInputElement): void {
    this.groups.set(withRef(this.groups(), event.option.value as NamedRef));
    input.value = '';
    this.groupOptions.set([]);
  }

  protected addMember(event: MatAutocompleteSelectedEvent, input: HTMLInputElement): void {
    const member = event.option.value as GrantCandidate;
    this.members.set(withRef(this.members(), { id: member.id, name: member.name }));
    input.value = '';
    this.memberOptions.set([]);
  }

  protected removeGroup(id: string): void {
    this.groups.set(withoutRef(this.groups(), id));
  }

  protected removeMember(id: string): void {
    this.members.set(withoutRef(this.members(), id));
  }

  protected async save(): Promise<void> {
    this.saving.set(true);
    this.errorKey.set(null);
    const value = this.form.getRawValue();
    const rule = this.data.rule;
    try {
      const saved = await this.api.invoke(updateNotificationRule, {
        id: this.data.configId,
        event: rule.event,
        'If-Match': `"${rule.version}"`,
        body: {
          recipientKinds: this.kinds.filter((kind) => value.kinds[kind]),
          roleIds: value.roleIds,
          groupIds: this.groups().map((g) => g.id),
          accountIds: this.members().map((m) => m.id),
        },
      });
      this.dialogRef.close(saved);
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, {}));
    } finally {
      this.saving.set(false);
    }
  }
}

/**
 * "Notifications" tab of a configuration (L04-US07): for each ticket
 * event, whom it notifies on the projects using the configuration; edited
 * in a dialog. The author of a change is never notified of it. Changes go
 * to the draft.
 */
@Component({
  selector: 'app-notifications-tab',
  imports: [ErrorState, MatButtonModule, MatIconModule, MatProgressBarModule, MatTableModule, TranslocoPipe],
  styleUrl: './notifications-tab.scss',
  templateUrl: './notifications-tab.html',
})
export class NotificationsTab {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);
  protected readonly context = inject(ConfigContext);

  protected readonly columns = ['event', 'recipients', 'actions'];
  protected readonly configId = computed(() => this.context.config()?.id);
  protected readonly rules = resource({
    params: () => this.configId(),
    loader: ({ params }) => this.api.invoke(listNotificationRules, { id: params }),
  });
  protected readonly items = computed(() => (this.rules.hasValue() ? this.rules.value().items : []));
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);

  protected recipients(rule: NotificationRule): Recipient[] {
    const references = this.rules.hasValue() ? this.rules.value().references : null;
    return recipientsOf(rule, references, (labels) => resolveLabel(labels, this.locale.language()));
  }

  protected edit(rule: NotificationRule): void {
    const data: NotificationDialogData = {
      configId: this.configId()!,
      rule,
      references: this.rules.hasValue() ? this.rules.value().references : null,
    };
    this.dialog
      .open<NotificationDialog, NotificationDialogData, NotificationRuleList>(NotificationDialog, {
        data,
        width: 'min(640px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe(async (saved) => {
        if (!saved) return;
        this.rules.set(saved);
        const event = this.transloco.translate(`config.notifications.events.${rule.event}`);
        this.notice.set({ ok: true, key: 'config.notifications.saved', params: { event } });
        const id = this.configId();
        if (id) this.context.config.set(await this.api.invoke(getProjectConfig, { id }));
      });
  }
}
