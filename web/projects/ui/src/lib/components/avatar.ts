import { Component, computed, input } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';

/** The initials of a name: the first letter of its first two words. */
export function initials(name: string): string {
  return name
    .split(/\s+/)
    .filter((w) => /^\p{L}/u.test(w))
    .slice(0, 2)
    .map((w) => w[0].toUpperCase())
    .join('');
}

/**
 * A person's avatar: their initials in a tonal circle, their name for
 * screen readers. Without a person, a dashed circle reads "Unassigned".
 */
@Component({
  selector: 'ui-avatar',
  imports: [TranslocoPipe],
  template: `
    @if (name(); as name) {
      <span class="avatar" [attr.data-size]="size()" [attr.title]="name">
        <span aria-hidden="true">{{ letters() }}</span>
        <span class="cdk-visually-hidden">{{ name }}</span>
      </span>
    } @else {
      <span class="avatar none" [attr.data-size]="size()" [attr.title]="'avatar.nobody' | transloco">
        <span aria-hidden="true">?</span>
        <span class="cdk-visually-hidden">{{ 'avatar.nobody' | transloco }}</span>
      </span>
    }
  `,
  styleUrl: './avatar.scss',
})
export class Avatar {
  /** The person's name; none: nobody. */
  readonly name = input<string | null | undefined>(null);
  readonly size = input<'small' | 'medium'>('small');

  protected readonly letters = computed(() => initials(this.name() ?? ''));
}
