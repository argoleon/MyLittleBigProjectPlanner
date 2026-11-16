import { XhrFactory } from '@angular/common';
import { HttpBackend, HttpClient, HttpEventType, HttpXhrBackend } from '@angular/common/http';
import { Injectable, InjectionToken, inject } from '@angular/core';
import { filter, lastValueFrom, tap } from 'rxjs';

import { Api } from './generated/api';
import { completeIssueAttachment } from './generated/fn/issues/complete-issue-attachment';
import { createIssueAttachment } from './generated/fn/issues/create-issue-attachment';
import { IssueAttachment } from './generated/models/issue-attachment';

/**
 * The HTTP backend of uploads to the storage: XMLHttpRequest, the only one
 * that reports upload progress (the API itself goes through `fetch`).
 * Tests provide the testing backend instead.
 */
export const UPLOAD_BACKEND = new InjectionToken<HttpBackend>('UPLOAD_BACKEND', {
  providedIn: 'root',
  factory: () => new HttpXhrBackend(inject(XhrFactory)),
});

/** The file could not be sent to the storage (network, refused). */
export class UploadFailed extends Error {
  constructor(readonly status: number) {
    super(`upload failed (${status})`);
    this.name = 'UploadFailed';
  }
}

/**
 * Attaches files to tickets (L05-US05): announces the file to the API,
 * sends it straight to the storage with the presigned URL the API gives
 * (same origin, no API in between), then tells the API it is there, which
 * queues its antivirus scan.
 *
 * The upload goes without the application's interceptors: no session nor
 * CSRF header reaches the storage, the signature of the URL is the only
 * credential.
 */
@Injectable({ providedIn: 'root' })
export class AttachmentUploader {
  private readonly api = inject(Api);
  private readonly storage = new HttpClient(inject(UPLOAD_BACKEND));

  /**
   * Attaches `file` to the ticket `issueId`; `progress` hears the share
   * sent, from 0 to 1. Fails with an `ApiProblem` (too large, over quota…)
   * or an {@link UploadFailed}.
   */
  async upload(issueId: string, file: File, progress: (sent: number) => void = () => undefined): Promise<IssueAttachment> {
    const announced = await this.api.invoke(createIssueAttachment, {
      idOrKey: issueId,
      body: { name: file.name, size: file.size, mediaType: file.type || undefined },
    });
    progress(0);
    try {
      await lastValueFrom(
        this.storage.put(announced.uploadUrl, file, { observe: 'events', reportProgress: true, responseType: 'text' }).pipe(
          tap((event) => {
            if (event.type === HttpEventType.UploadProgress && event.total) progress(event.loaded / event.total);
          }),
          filter((event) => event.type === HttpEventType.Response),
        ),
      );
    } catch (error) {
      throw new UploadFailed((error as { status?: number }).status ?? 0);
    }
    progress(1);
    return this.api.invoke(completeIssueAttachment, { idOrKey: issueId, attachmentId: announced.attachment.id });
  }
}
