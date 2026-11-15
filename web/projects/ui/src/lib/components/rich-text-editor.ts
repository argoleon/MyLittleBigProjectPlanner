import {
  Component,
  DestroyRef,
  ElementRef,
  ViewEncapsulation,
  afterNextRender,
  forwardRef,
  inject,
  input,
  signal,
  viewChild,
} from '@angular/core';
import { ControlValueAccessor, NG_VALUE_ACCESSOR } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';
import { AnyExtension, ChainedCommands, Editor, JSONContent, mergeAttributes } from '@tiptap/core';
import { Mention } from '@tiptap/extension-mention';
import { StarterKit } from '@tiptap/starter-kit';
import { SuggestionKeyDownProps, SuggestionOptions, SuggestionProps } from '@tiptap/suggestion';

/** A rich text document, as the editor (Tiptap) and the API exchange it. */
export type RichTextDoc = JSONContent;

/** Something to insert after `@` (a person) or `#` (a ticket). */
export interface Suggestion {
  /** A person: their account id; a ticket: its key. */
  readonly id: string;
  /** What the document shows: a name, or the key. */
  readonly label: string;
  /** More to tell it apart in the list (an email, a summary). */
  readonly detail?: string;
}

/** Finds the suggestions for what was typed after `@` or `#`. */
export type SuggestionSource = (query: string) => Promise<readonly Suggestion[]>;

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

/** The suggestions shown under the caret. */
interface SuggestionList {
  readonly items: readonly Suggestion[];
  readonly index: number;
  readonly command: (item: Suggestion) => void;
  /** Where to show it, in the frame. */
  readonly top: number;
  readonly left: number;
}

/**
 * Rich text field (Tiptap, MIT core, starter kit and mention only):
 * paragraphs, headings, lists, quotes, code and links, the nodes the
 * server accepts; with `mentions`, `@` inserts a person, with `issueRefs`,
 * `#` a ticket, each from a list the keyboard drives. A form control whose
 * value is the document, `null` when empty.
 */
@Component({
  selector: 'ui-rich-text-editor',
  imports: [MatButtonModule, MatIconModule, TranslocoPipe],
  providers: [{ provide: NG_VALUE_ACCESSOR, useExisting: forwardRef(() => RichTextEditor), multi: true }],
  templateUrl: './rich-text-editor.html',
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
  /** The people `@` offers; none: no mentions. Read when the editor starts. */
  readonly mentions = input<SuggestionSource | null>(null);
  /** The tickets `#` offers; none: no references. Read when the editor starts. */
  readonly issueRefs = input<SuggestionSource | null>(null);

  protected readonly actions = ACTIONS;
  protected readonly linkAction: Action = { id: 'link', icon: 'link', run: (c) => c, active: ['link'] };
  private readonly id = RichTextEditor.nextId++;
  protected readonly labelId = `ui-rich-text-${this.id}-label`;
  protected readonly contentId = `ui-rich-text-${this.id}`;
  protected readonly linkInputId = `ui-rich-text-${this.id}-link`;
  protected readonly listId = `ui-rich-text-${this.id}-suggestions`;

  private readonly host = viewChild.required<ElementRef<HTMLElement>>('content');
  private readonly frame = viewChild.required<ElementRef<HTMLElement>>('frame');
  private readonly linkInput = viewChild<ElementRef<HTMLInputElement>>('linkInput');
  private editor: Editor | null = null;
  private pending: RichTextDoc | null = null;
  /** Bumped on every change of the document or selection: the toolbar reads it. */
  private readonly revision = signal(0);
  protected readonly disabled = signal(false);
  protected readonly linking = signal(false);
  protected readonly linkError = signal(false);
  protected readonly suggestions = signal<SuggestionList | null>(null);
  private onChange: (value: RichTextDoc | null) => void = () => undefined;
  private onTouched: () => void = () => undefined;

  constructor() {
    afterNextRender(() => {
      const extensions: AnyExtension[] = [
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
      ];
      const mentions = this.mentions();
      if (mentions) {
        extensions.push(
          Mention.configure({
            HTMLAttributes: { class: 'mention' },
            suggestion: this.suggestionOf('@', mentions),
          }),
        );
      }
      const issueRefs = this.issueRefs();
      if (issueRefs) {
        extensions.push(
          Mention.extend({ name: 'issueRef' }).configure({
            HTMLAttributes: { class: 'issue-ref' },
            // The key alone, as the server renders it.
            renderText: ({ node }) => String(node.attrs['label'] ?? node.attrs['id']),
            renderHTML: ({ options, node }) => [
              'span',
              mergeAttributes({ 'data-type': 'issueRef' }, options.HTMLAttributes),
              String(node.attrs['label'] ?? node.attrs['id']),
            ],
            suggestion: this.suggestionOf('#', issueRefs),
          }),
        );
      }
      this.editor = new Editor({
        element: this.host().nativeElement,
        extensions,
        content: this.pending,
        editable: !this.disabled(),
        editorProps: {
          attributes: {
            id: this.contentId,
            class: 'content',
            role: 'textbox',
            'aria-multiline': 'true',
            'aria-labelledby': this.labelId,
            ...(mentions || issueRefs ? { 'aria-autocomplete': 'list' } : {}),
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

  protected optionId(index: number): string {
    return `${this.listId}-${index}`;
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

  /** The suggestion of `char` from `source`, shown in the component's own list. */
  private suggestionOf(char: string, source: SuggestionSource): Omit<SuggestionOptions<Suggestion>, 'editor'> {
    return {
      char,
      items: ({ query }) => source(query).then((items) => [...items]),
      render: () => ({
        onStart: (props) => this.showSuggestions(props),
        onUpdate: (props) => this.showSuggestions(props),
        onKeyDown: (props) => this.suggestionKey(props),
        onExit: () => this.hideSuggestions(),
      }),
    };
  }

  private showSuggestions(props: SuggestionProps<Suggestion>): void {
    const caret = props.clientRect?.();
    const frame = this.frame().nativeElement.getBoundingClientRect();
    const previous = this.suggestions();
    this.suggestions.set({
      items: props.items,
      index: previous && previous.items === props.items ? previous.index : 0,
      command: (item) => props.command({ id: item.id, label: item.label }),
      top: caret ? caret.bottom - frame.top + 4 : 0,
      left: caret ? Math.max(0, caret.left - frame.left) : 0,
    });
    this.pointAt(this.suggestions());
  }

  private suggestionKey({ event }: SuggestionKeyDownProps): boolean {
    const list = this.suggestions();
    if (!list) return false;
    const count = list.items.length;
    switch (event.key) {
      case 'ArrowDown':
      case 'ArrowUp': {
        if (count === 0) return true;
        const step = event.key === 'ArrowDown' ? 1 : count - 1;
        const next = { ...list, index: (list.index + step) % count };
        this.suggestions.set(next);
        this.pointAt(next);
        return true;
      }
      case 'Enter':
      case 'Tab':
        if (count === 0) return false;
        list.command(list.items[list.index]);
        return true;
      case 'Escape':
        this.hideSuggestions();
        return true;
      default:
        return false;
    }
  }

  private hideSuggestions(): void {
    this.suggestions.set(null);
    this.pointAt(null);
  }

  /** Tells assistive technologies which suggestion is current, the focus staying in the text. */
  private pointAt(list: SuggestionList | null): void {
    const dom = this.editor?.view.dom;
    if (!dom) return;
    if (list && list.items.length > 0) {
      dom.setAttribute('aria-controls', this.listId);
      dom.setAttribute('aria-activedescendant', this.optionId(list.index));
    } else {
      dom.removeAttribute('aria-controls');
      dom.removeAttribute('aria-activedescendant');
    }
  }
}
