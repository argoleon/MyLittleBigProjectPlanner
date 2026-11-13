import { Component } from '@angular/core';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';
import { VeiledNode } from 'ui';

/**
 * Whether an item of a list of nodes is a veiled one: a confidential
 * project the user has no access to, listed without name or detail because
 * they see one of its parents.
 */
export function isVeiled(item: object): item is VeiledNode {
  return 'veiled' in item && item.veiled === true;
}

/**
 * What stands for the name of a veiled node: a lock, labelled for screen
 * readers, and "Confidential project".
 */
@Component({
  selector: 'app-veiled-name',
  imports: [MatIconModule, TranslocoPipe],
  template: `
    <mat-icon class="lock" role="img" aria-hidden="false" [attr.aria-label]="'hierarchy.veiled.lock' | transloco"
      >lock</mat-icon
    >
    <span class="name">{{ 'hierarchy.veiled.name' | transloco }}</span>
  `,
  styleUrl: './veiled.scss',
})
export class VeiledName {}
