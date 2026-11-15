import { Component, computed, effect, inject, input, output, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AttachmentUploader,
  ConfirmDialog,
  ConfirmDialogData,
  ErrorState,
  InstantPipe,
  Issue,
  IssueAttachment,
  LocaleService,
  UploadFailed,
  deleteIssueAttachment,
  errorTranslationKey,
  formatFileSize,
  isApiProblem,
  listIssueAttachments,
} from 'ui';

/** How often the list is read again while files are being scanned. */
export const SCAN_POLL_MS = 3000;

/** A file being sent. */
interface Upload {
  key: number;
  name: string;
  size: number;
  /** Share sent, from 0 to 1. */
  sent: number;
  errorKey: string | null;
}

/** The translation key of why an upload failed. */
function failureOf(error: unknown): string {
  if (error instanceof UploadFailed) return 'issue.attachments.uploadFailed';
  if (isApiProblem(error)) return error.translationKey;
  return errorTranslationKey('unknown');
}

/** The icon of a file, by its type. */
function iconOf(attachment: IssueAttachment): string {
  const type = attachment.mediaType;
  if (type.startsWith('image/')) return 'image';
  if (type === 'application/pdf') return 'picture_as_pdf';
  if (type.startsWith('video/')) return 'movie';
  if (type.startsWith('audio/')) return 'audio_file';
  if (type.startsWith('text/')) return 'description';
  if (/zip|compressed|tar/.test(type)) return 'folder_zip';
  return 'draft';
}

/**
 * The files attached to a ticket (L05-US05): dropped or picked, sent
 * straight to the storage with their progress, scanned by the antivirus
 * (the list is read again until they are), then downloadable, images with
 * their thumbnail. Their uploader, or a moderator, removes them.
 */
@Component({
  selector: 'app-issue-attachments',
  imports: [ErrorState, InstantPipe, MatButtonModule, MatIconModule, MatProgressBarModule, TranslocoPipe],
  templateUrl: './issue-attachments.html',
  styleUrl: './issue-attachments.scss',
})
export class IssueAttachments {
  private readonly api = inject(Api);
  private readonly uploader = inject(AttachmentUploader);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);

  readonly issue = input.required<Issue>();
  /** The member may attach files (`edit` on the project). */
  readonly canAttach = input(false);
  /** A file was attached or removed: the history changed. */
  readonly changed = output<void>();

  protected readonly attachments = resource({
    params: () => this.issue().id,
    loader: ({ params }) => this.api.invoke(listIssueAttachments, { idOrKey: params }),
  });
  protected readonly count = computed(() => (this.attachments.hasValue() ? this.attachments.value().length : 0));
  protected readonly uploads = signal<Upload[]>([]);
  protected readonly over = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  private nextKey = 0;

  protected readonly iconOf = iconOf;

  constructor() {
    // Scans take seconds: read again until none is under way.
    effect((onCleanup) => {
      const scanning = this.attachments.hasValue() && this.attachments.value().some((a) => a.status === 'scanning');
      if (!scanning) return;
      const timer = setTimeout(() => this.attachments.reload(), SCAN_POLL_MS);
      onCleanup(() => clearTimeout(timer));
    });
  }

  protected size(bytes: number): string {
    return formatFileSize(bytes, this.locale.formattingLocale());
  }

  /** Where a file downloads from: the API redirects to the storage once it is clean. */
  protected contentUrl(attachment: IssueAttachment): string {
    return `/api/v1/issues/${this.issue().id}/attachments/${attachment.id}/content`;
  }

  protected thumbnailUrl(attachment: IssueAttachment): string {
    return `/api/v1/issues/${this.issue().id}/attachments/${attachment.id}/thumbnail`;
  }

  protected picked(input: HTMLInputElement): void {
    this.send(Array.from(input.files ?? []));
    input.value = '';
  }

  protected dragOver(event: DragEvent): void {
    if (!this.canAttach() || !event.dataTransfer?.types.includes('Files')) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = 'copy';
    this.over.set(true);
  }

  protected dropped(event: DragEvent): void {
    this.over.set(false);
    if (!this.canAttach()) return;
    event.preventDefault();
    this.send(Array.from(event.dataTransfer?.files ?? []));
  }

  /** Sends `files` one after the other, each with its progress. */
  async send(files: File[]): Promise<void> {
    this.errorKey.set(null);
    const rows = files.map((file) => ({ file, row: { key: this.nextKey++, name: file.name, size: file.size, sent: 0, errorKey: null } as Upload }));
    this.uploads.update((uploads) => [...uploads, ...rows.map((r) => r.row)]);
    const update = (key: number, change: Partial<Upload>) =>
      this.uploads.update((uploads) => uploads.map((u) => (u.key === key ? { ...u, ...change } : u)));
    for (const { file, row } of rows) {
      try {
        await this.uploader.upload(this.issue().id, file, (sent) => update(row.key, { sent }));
        this.uploads.update((uploads) => uploads.filter((u) => u.key !== row.key));
        this.attachments.reload();
        this.changed.emit();
      } catch (error) {
        update(row.key, { errorKey: failureOf(error) });
      }
    }
  }

  protected dismiss(upload: Upload): void {
    this.uploads.update((uploads) => uploads.filter((u) => u.key !== upload.key));
  }

  protected remove(attachment: IssueAttachment): void {
    const data: ConfirmDialogData = {
      titleKey: 'issue.attachments.removeTitle',
      messageKey: 'issue.attachments.removeMessage',
      params: { name: attachment.name },
      confirmKey: 'issue.attachments.remove',
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        this.errorKey.set(null);
        try {
          await this.api.invoke(deleteIssueAttachment, { idOrKey: this.issue().id, attachmentId: attachment.id });
          this.attachments.reload();
          this.changed.emit();
        } catch (error) {
          this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
        }
      });
  }
}
