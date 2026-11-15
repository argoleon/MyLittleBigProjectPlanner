/**
 * Tests of the attachment uploader (L05-US05), the API and the storage
 * mocked at the HTTP level: a file announced, sent to the presigned URL
 * with its progress and without the application's CSRF header, then said
 * uploaded; a refusal of the API, and a failed upload.
 */
import { HttpBackend, HttpEventType } from '@angular/common/http';
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';

import { ApiProblem } from './api-problem';
import { AttachmentUploader, UPLOAD_BACKEND, UploadFailed } from './attachment-uploader';
import { IssueAttachment } from './generated/models/issue-attachment';
import { provideApi } from './provide-api';

const ATTACHMENT: IssueAttachment = {
  id: 'f1',
  name: 'plan.pdf',
  size: 5,
  mediaType: 'application/pdf',
  status: 'pending',
  signature: null,
  uploadedBy: { id: 'a1', name: 'Lucas Bernard' },
  createdAt: '2026-11-15T09:00:00Z',
  hasThumbnail: false,
  canDelete: true,
};

const UPLOAD_URL = '/mlbpp-attachments/t1/uploads/f1?X-Amz-Signature=abc';

describe('AttachmentUploader', () => {
  function setUp() {
    document.cookie = 'mlbpp_csrf=csrf-token';
    TestBed.configureTestingModule({
      providers: [provideApi(), provideHttpClientTesting(), { provide: UPLOAD_BACKEND, useExisting: HttpBackend }],
    });
    return { uploader: TestBed.inject(AttachmentUploader), http: TestBed.inject(HttpTestingController) };
  }

  const next = async (http: HttpTestingController, match: (url: string, method: string) => boolean): Promise<TestRequest> => {
    for (let i = 0; i < 50; i++) {
      const [request] = http.match((r) => match(r.urlWithParams, r.method));
      if (request) return request;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    throw new Error('no matching request');
  };

  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('announces the file, sends it with its progress, then says it is uploaded', async () => {
    const { uploader, http } = setUp();
    const file = new File(['hello'], 'plan.pdf', { type: 'application/pdf' });
    const progress: number[] = [];
    const done = uploader.upload('i142', file, (sent) => progress.push(sent));

    const announce = await next(http, (url, method) => url === '/api/v1/issues/i142/attachments' && method === 'POST');
    expect(announce.request.body).toEqual({ name: 'plan.pdf', size: 5, mediaType: 'application/pdf' });
    announce.flush({ attachment: ATTACHMENT, uploadUrl: UPLOAD_URL }, { status: 201, statusText: 'Created' });

    const put = await next(http, (url, method) => url === UPLOAD_URL && method === 'PUT');
    expect(put.request.body).toBe(file);
    expect(put.request.headers.has('X-CSRF-Token')).toBe(false);
    put.event({ type: HttpEventType.UploadProgress, loaded: 2, total: 5 });
    put.flush('', { status: 200, statusText: 'OK' });

    const complete = await next(http, (url, method) => url === '/api/v1/issues/i142/attachments/f1/uploaded' && method === 'POST');
    complete.flush({ ...ATTACHMENT, status: 'scanning' });
    expect((await done).status).toBe('scanning');
    expect(progress).toEqual([0, 0.4, 1]);
  });

  it('passes on the refusal of the API, and a failed upload', async () => {
    const { uploader, http } = setUp();
    const file = new File(['x'.repeat(10)], 'video.mp4');
    const refused = uploader.upload('i142', file).catch((error: unknown) => error);
    const announce = await next(http, (url) => url.endsWith('/attachments'));
    expect(announce.request.body).toEqual({ name: 'video.mp4', size: 10, mediaType: undefined });
    announce.flush(
      { type: 'about:blank', title: 'Too large', status: 413, code: 'attachment.too_large' },
      { status: 413, statusText: 'Payload Too Large' },
    );
    const problem = await refused;
    expect(problem instanceof ApiProblem && problem.code).toBe('attachment.too_large');

    const failed = uploader.upload('i142', file).catch((error: unknown) => error);
    (await next(http, (url) => url.endsWith('/attachments'))).flush(
      { attachment: ATTACHMENT, uploadUrl: UPLOAD_URL },
      { status: 201, statusText: 'Created' },
    );
    (await next(http, (_, method) => method === 'PUT')).flush('Forbidden', { status: 403, statusText: 'Forbidden' });
    const error = await failed;
    expect(error instanceof UploadFailed && error.status).toBe(403);
  });
});
