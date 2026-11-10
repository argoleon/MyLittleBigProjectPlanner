import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { firstValueFrom } from 'rxjs';

import { ApiProblem } from './api-problem';
import { ApiConfiguration } from './generated/api-configuration';
import { ApiOptions, provideApi } from './provide-api';

function setUp(options?: ApiOptions): { http: HttpClient; backend: HttpTestingController } {
  TestBed.configureTestingModule({
    providers: [provideApi(options), provideHttpClientTesting()],
  });
  return { http: TestBed.inject(HttpClient), backend: TestBed.inject(HttpTestingController) };
}

/** The rejection of `promise`, failing the test if it resolves. */
async function rejection(promise: Promise<unknown>): Promise<unknown> {
  try {
    await promise;
  } catch (error) {
    return error;
  }
  throw new Error('expected a rejection');
}

describe('provideApi', () => {
  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('uses the same origin by default', () => {
    setUp();
    expect(TestBed.inject(ApiConfiguration).rootUrl).toBe('');
  });

  it('takes the API origin without trailing slashes', () => {
    setUp({ rootUrl: 'https://api.example.test//' });
    expect(TestBed.inject(ApiConfiguration).rootUrl).toBe('https://api.example.test');
  });

  it('sends cookies to the API only', async () => {
    const { http, backend } = setUp();
    const apiCall = firstValueFrom(http.get('/api/v1/ping'));
    const asset = firstValueFrom(http.get('/i18n/en.json'));

    const apiRequest = backend.expectOne('/api/v1/ping');
    const assetRequest = backend.expectOne('/i18n/en.json');
    expect(apiRequest.request.withCredentials).toBe(true);
    expect(assetRequest.request.withCredentials).toBe(false);

    apiRequest.flush({ status: 'ok' });
    assetRequest.flush({});
    await Promise.all([apiCall, asset]);
  });

  it('recognises API requests on another origin', () => {
    const { http, backend } = setUp({ rootUrl: 'https://api.example.test' });
    http.get('https://api.example.test/api/v1/ping').subscribe();
    http.get('/api/v1/ping').subscribe();
    expect(backend.expectOne('https://api.example.test/api/v1/ping').request.withCredentials).toBe(true);
    expect(backend.expectOne('/api/v1/ping').request.withCredentials).toBe(false);
  });

  it('turns API error responses into ApiProblem', async () => {
    const { http, backend } = setUp();
    const call = rejection(firstValueFrom(http.get('/api/v1/reference-items/x')));
    backend.expectOne('/api/v1/reference-items/x').flush(
      {
        type: 'urn:mlbpp:problem:not_found',
        title: 'Resource not found',
        status: 404,
        code: 'not_found',
      },
      { status: 404, statusText: 'Not Found', headers: { 'content-type': 'application/problem+json' } },
    );
    const error = await call;
    expect(error).toBeInstanceOf(ApiProblem);
    expect((error as ApiProblem).code).toBe('not_found');
    expect((error as ApiProblem).translationKey).toBe('errors.not_found');
  });

  it('turns network failures into ApiProblem', async () => {
    const { http, backend } = setUp();
    const call = rejection(firstValueFrom(http.get('/api/v1/ping')));
    backend.expectOne('/api/v1/ping').error(new ProgressEvent('error'));
    const error = (await call) as ApiProblem;
    expect(error.code).toBe('network');
    expect(error.status).toBe(0);
  });

  it('leaves errors of other requests untouched', async () => {
    const { http, backend } = setUp();
    const call = rejection(firstValueFrom(http.get('/i18n/de.json')));
    backend.expectOne('/i18n/de.json').flush('missing', { status: 404, statusText: 'Not Found' });
    expect(await call).toBeInstanceOf(HttpErrorResponse);
  });
});
