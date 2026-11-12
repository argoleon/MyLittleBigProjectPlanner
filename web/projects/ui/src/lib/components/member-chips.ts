import { Component, input } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';

/** Kind of account of a member in a tenant. */
export type AccountKind = 'internal' | 'external';

/** Where a member, or an invitee, stands. */
export type MemberState = 'active' | 'suspended' | 'invited' | 'expired';

/** Label of a member's account type (`--app-account-*` tokens). */
@Component({
  selector: 'ui-account-type-chip',
  imports: [TranslocoPipe],
  template: `
    <span class="chip" [attr.data-kind]="type()">
      <span class="cdk-visually-hidden">{{ 'member.accountType.label' | transloco }} </span
      >{{ 'member.accountType.' + type() | transloco }}
    </span>
  `,
  styleUrls: ['./chip.scss', './member-chips.scss'],
})
export class AccountTypeChip {
  readonly type = input.required<AccountKind>();
}

/** Label of a member's status (`--app-member-*` tokens). */
@Component({
  selector: 'ui-member-status-chip',
  imports: [TranslocoPipe],
  template: `
    <span class="chip" [attr.data-state]="state()">
      <span class="cdk-visually-hidden">{{ 'member.status.label' | transloco }} </span
      >{{ 'member.status.' + state() | transloco }}
    </span>
  `,
  styleUrls: ['./chip.scss', './member-chips.scss'],
})
export class MemberStatusChip {
  readonly state = input.required<MemberState>();
}
