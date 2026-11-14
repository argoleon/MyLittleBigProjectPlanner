import { Component, ViewEncapsulation, input } from '@angular/core';

/**
 * Rich text as the server renders it (`descriptionHtml`…): HTML it built
 * from an allow-list and escaped, shown through Angular's sanitizer too.
 */
@Component({
  selector: 'ui-rich-text',
  template: `<div class="ui-rich-text" [innerHTML]="html()"></div>`,
  styleUrl: './rich-text.scss',
  // The HTML is not created by the template: its elements get no
  // encapsulation attribute, so the styles are scoped by class instead.
  encapsulation: ViewEncapsulation.None,
})
export class RichText {
  readonly html = input.required<string>();
}
