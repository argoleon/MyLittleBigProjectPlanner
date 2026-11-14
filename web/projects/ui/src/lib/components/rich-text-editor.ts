import { Component, DestroyRef, ElementRef, ViewEncapsulation, afterNextRender, forwardRef, inject, input, signal, viewChild } from '@angular/core';
import { ControlValueAccessor, NG_VALUE_ACCESSOR } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';
import { ChainedCommands, Editor, JSONContent } from '@tiptap/core';
import { StarterKit } from '@tiptap/starter-kit';

/** A rich text document, as the editor (Tiptap) and the API exchange it. */
export type RichTextDoc = JSONContent;

/**
 * Whether a link may point to `href`: web pages and email addresses only,
 * as the server accepts (`domain::rich_text::is_allowed_href`).
 */
export function isAllowedHref(href: string): boolean {
  return /^(https?:\/\/[^\s]+|mailto:[^\s]+)$/i.test(href.trim());
}

/** A button of the toolbar. */
interface Action {
  readonly id: string;
  readonly icon: string;
  readonly run: (chain: ChainedCommands) => ChainedCommands;
  /** The mark or node it toggles, to show it pressed. */
  readonly active: readonly [string, Record<string, unknown>?];
}

const ACTIONS: readonly Action[] = [
  { id: 'bold', icon: 'format_bold', run: (c) => c.toggleBold(), active: ['bold'] },
  { id: 'italic', icon: 'format_italic', run: (c) => c.toggleItalic(), active: ['italic'] },
  { id: 'strike', icon: 'format_strikethrough', run: (c) => c.toggleStrike(), active: ['strike'] },
  { id: 'code', icon: 'code', run: (c) => c.toggleCode(), active: ['code'] },
  { id: 'heading', icon: 'title', run: (c) => c.toggleHeading({ level: 3 }), active: ['heading', { level: 3 }] },
  { id: 'bulletList', icon: 'format_list_bulleted', run: (c) => c.toggleBulletList(), active: ['bulletList'] },
  { id: 'orderedList', icon: 'format_list_numbered', run: (c) => c.toggleOrderedList(), active: ['orderedList'] },
  { id: 'blockquote', icon: 'format_quote', run: (c) => c.toggleBlockquote(), active: ['blockquote'] },
  { id: 'codeBlock', icon: 'code_blocks', run: (c) => c.toggleCodeBlock(), active: ['codeBlock'] },
];

/**
 * Rich text field (Tiptap, MIT core and starter kit only): paragraphs,
 * headings, lists, quotes, code and links, the nodes the server accepts.
 * A form control whose value is the document, `null` when empty.
 */
@Component({
  selector: 'ui-rich-text-editor',
  imports: [MatButtonModule, MatIconModule, TranslocoPipe],
  providers: [{ provide: NG_VALUE_ACCESSOR, useExisting: forwardRef(() => RichTextEditor), multi: true }],
  template: `
    <div class="frame" [class.disabled]="disabled()" [class.invalid]="invalid()">
      <span class="label" [id]="labelId">{{ label() }}</span>
      <div class="toolbar" role="toolbar" [attr.aria-label]="'richText.toolbar' | transloco" [attr.aria-controls]="contentId">
        @for (action of actions; track action.id) {
          <button
            matIconButton
            type="button"
            [class]="'action ' + action.id"
            [disabled]="disabled()"
            [attr.aria-label]="'richText.' + action.id | transloco"
            [attr.aria-pressed]="isActive(action)"
            (click)="apply(action)"
          >
            <mat-icon aria-hidden="true">{{ action.icon }}</mat-icon>
          </button>
        }
        <button
          matIconButton
          type="button"
          class="action link"
          [disabled]="disabled()"
          [attr.aria-label]="'richText.link' | transloco"
          [attr.aria-pressed]="isActive(linkAction)"
          [attr.aria-expanded]="linking()"
          (click)="toggleLink()"
        >
          <mat-icon aria-hidden="true">link</mat-icon>
        </button>
      </div>
      @if (linking()) {
        <div class="link-form">
          <label class="link-label" [for]="linkInputId">{{ 'richText.linkAddress' | transloco }}</label>
          <input
            #linkInput
            class="link-input"
            type="url"
            [id]="linkInputId"
            [attr.aria-invalid]="linkError()"
            [attr.aria-describedby]="linkError() ? linkInputId + '-error' : null"
            (keydown.enter)="$event.preventDefault(); setLink(linkInput.value)"
            (keydown.escape)="linking.set(false)"
          />
          <button matButton type="button" class="link-apply" (click)="setLink(linkInput.value)">{{ 'richText.linkApply' | transloco }}</button>
          @if (linkError()) {
            <p class="link-error" role="alert" [id]="linkInputId + '-error'">{{ 'richText.linkInvalid' | transloco }}</p>
          }
        </div>
      }
      <div #content class="content-host"></div>
    </div>
  `,
  styleUrl: './rich-text-editor.scss',
  // Tiptap renders the editable area outside Angular's view: the styles are
  // scoped by the element instead of the encapsulation attribute.
  encapsulation: ViewEncapsulation.None,
})
export class RichTextEditor implements ControlValueAccessor {
  private static nextId = 0;

  /** Visible label of the field. */
  readonly label = input.required<string>();
  /** Shown in error (the form control's errors are the caller's to show). */
  readonly invalid = input(false);

  protected readonly actions = ACTIONS;
  protected readonly linkAction: Action = { id: 'link', icon: 'link', run: (c) => c, active: ['link'] };
  private readonly id = RichTextEditor.nextId++;
  protected readonly labelId = `ui-rich-text-${this.id}-label`;
  protected readonly contentId = `ui-rich-text-${this.id}`;
  protected readonly linkInputId = `ui-rich-text-${this.id}-link`;

  private readonly host = viewChild.required<ElementRef<HTMLElement>>('content');
  private readonly linkInput = viewChild<ElementRef<HTMLInputElement>>('linkInput');
  private editor: Editor | null = null;
  private pending: RichTextDoc | null = null;
  /** Bumped on every change of the document or selection: the toolbar reads it. */
  private readonly revision = signal(0);
  protected readonly disabled = signal(false);
  protected readonly linking = signal(false);
  protected readonly linkError = signal(false);
  private onChange: (value: RichTextDoc | null) => void = () => undefined;
  private onTouched: () => void = () => undefined;

  constructor() {
    afterNextRender(() => {
      this.editor = new Editor({
        element: this.host().nativeElement,
        extensions: [
          StarterKit.configure({
            heading: { levels: [1, 2, 3, 4] },
            underline: false,
            // No empty paragraph added after a list or a quote: the document
            // stored is what was written.
            trailingNode: false,
            link: {
              openOnClick: false,
              autolink: true,
              defaultProtocol: 'https',
              isAllowedUri: (url) => isAllowedHref(url),
            },
          }),
        ],
        content: this.pending,
        editable: !this.disabled(),
        editorProps: {
          attributes: {
            id: this.contentId,
            class: 'content',
            role: 'textbox',
            'aria-multiline': 'true',
            'aria-labelledby': this.labelId,
          },
        },
        onUpdate: ({ editor }) => this.onChange(editor.isEmpty ? null : editor.getJSON()),
        onTransaction: () => this.revision.update((r) => r + 1),
        onBlur: () => this.onTouched(),
      });
    });
    inject(DestroyRef).onDestroy(() => this.editor?.destroy());
  }

  writeValue(value: RichTextDoc | null): void {
    this.pending = value ?? null;
    this.editor?.commands.setContent(value ?? '', { emitUpdate: false });
  }

  registerOnChange(fn: (value: RichTextDoc | null) => void): void {
    this.onChange = fn;
  }

  registerOnTouched(fn: () => void): void {
    this.onTouched = fn;
  }

  setDisabledState(disabled: boolean): void {
    this.disabled.set(disabled);
    this.editor?.setEditable(!disabled, false);
  }

  protected isActive(action: Action): boolean {
    this.revision();
    return this.editor?.isActive(...action.active) ?? false;
  }

  protected apply(action: Action): void {
    if (!this.editor) return;
    action.run(this.editor.chain().focus()).run();
  }

  protected toggleLink(): void {
    if (!this.editor) return;
    if (this.editor.isActive('link')) {
      this.editor.chain().focus().extendMarkRange('link').unsetLink().run();
      return;
    }
    this.linkError.set(false);
    this.linking.update((open) => !open);
    if (this.linking()) setTimeout(() => this.linkInput()?.nativeElement.focus());
  }

  protected setLink(href: string): void {
    if (!this.editor) return;
    if (!isAllowedHref(href)) {
      this.linkError.set(true);
      return;
    }
    this.editor.chain().focus().extendMarkRange('link').setLink({ href: href.trim() }).run();
    this.linking.set(false);
  }
}
