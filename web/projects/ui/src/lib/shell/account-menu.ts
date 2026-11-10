import { Component, input, output } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDividerModule } from '@angular/material/divider';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { TranslocoPipe } from '@jsverse/transloco';

/**
 * Icon button of the top app bar opening the signed-in member's menu: who
 * is signed in, two-step verification, and signing out.
 */
@Component({
  selector: 'ui-account-menu',
  imports: [MatButtonModule, MatDividerModule, MatIconModule, MatMenuModule, TranslocoPipe],
  styleUrl: './account-menu.scss',
  template: `
    <button
      matIconButton
      type="button"
      [matMenuTriggerFor]="menu"
      [attr.aria-label]="'auth.account.menu' | transloco: { name: displayName() }"
    >
      <mat-icon aria-hidden="true">account_circle</mat-icon>
    </button>
    <mat-menu #menu="matMenu" xPosition="before">
      <div class="who">
        <span class="name">{{ displayName() }}</span>
        <span class="email">{{ email() }}</span>
      </div>
      <mat-divider />
      <button mat-menu-item type="button" (click)="security.emit()">
        <mat-icon aria-hidden="true">security</mat-icon>
        <span>{{ 'auth.account.security' | transloco }}</span>
      </button>
      <button mat-menu-item type="button" (click)="signOut.emit()">
        <mat-icon aria-hidden="true">logout</mat-icon>
        <span>{{ 'auth.account.signOut' | transloco }}</span>
      </button>
    </mat-menu>
  `,
})
export class AccountMenu {
  readonly displayName = input.required<string>();
  readonly email = input.required<string>();
  readonly signOut = output<void>();
  /** Two-step verification was picked. */
  readonly security = output<void>();
}
