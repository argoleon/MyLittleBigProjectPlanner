import { Component, ViewEncapsulation, inject, input } from '@angular/core';
import { Router } from '@angular/router';

/**
 * Rich text as the server renders it (`descriptionHtml`, comments…): HTML
 * it built from an allow-list and escaped, shown through Angular's
 * sanitizer too. Links to tickets (`a.issue-ref`) open in the application.
 */
@Component({
  selector: 'ui-rich-text',
  template: `<div class="ui-rich-text" [innerHTML]="html()"></div>`,
  // Delegated from its links (keyboard included: Enter on a link clicks it).
  host: { '(click)': 'follow($event)' },
  styleUrl: './rich-text.scss',
  // The HTML is not created by the template: its elements get no
  // encapsulation attribute, so the styles are scoped by class instead.
  encapsulation: ViewEncapsulation.None,
})
export class RichText {
  private readonly router = inject(Router, { optional: true });

  readonly html = input.required<string>();

  /** A link to a ticket opens it in the application, unless asked otherwise (new tab…). */
  protected follow(event: MouseEvent): void {
    const link = (event.target as Element | null)?.closest('a.issue-ref');
    if (!link || !this.router || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    const href = link.getAttribute('href');
    if (!href?.startsWith('/issues/')) return;
    event.preventDefault();
    void this.router.navigateByUrl(href);
  }
}
