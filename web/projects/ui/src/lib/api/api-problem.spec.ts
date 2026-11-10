import { HttpErrorResponse, HttpHeaders } from '@angular/common/http';

import {
  ApiProblem,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  isErrorCode,
} from './api-problem';
import { ERROR_CODE } from './generated/models/error-code-array';

function httpError(status: number, error: unknown, statusText = 'Error'): HttpErrorResponse {
  return new HttpErrorResponse({
    status,
    statusText,
    error,
    url: '/api/v1/reference-items',
    headers: new HttpHeaders({ 'content-type': 'application/problem+json' }),
  });
}

describe('error translation keys', () => {
  it('maps every code of the API catalog to errors.<code>', () => {
    expect(ERROR_CODE.length).toBeGreaterThan(0);
    for (const code of ERROR_CODE) {
      expect(isErrorCode(code)).toBe(true);
      expect(errorTranslationKey(code)).toBe(`errors.${code}`);
    }
    expect(errorTranslationKey('reference_item.code_taken')).toBe('errors.reference_item.code_taken');
  });

  it('maps client codes and unknown codes to a key that exists', () => {
    expect(errorTranslationKey('network')).toBe('errors.network');
    expect(errorTranslationKey('unknown')).toBe('errors.unknown');
    expect(errorTranslationKey('some.future_code')).toBe('errors.unknown');
    expect(isErrorCode('some.future_code')).toBe(false);
    expect(isErrorCode(42)).toBe(false);
  });

  it('maps field error codes to errors.field.<code>', () => {
    expect(fieldErrorTranslationKey('required')).toBe('errors.field.required');
  });
});

describe('ApiProblem.fromHttpError', () => {
  it('reads a Problem Details body', () => {
    const problem = ApiProblem.fromHttpError(
      httpError(400, {
        type: 'urn:mlbpp:problem:validation_failed',
        title: 'The request contains invalid fields',
        status: 400,
        code: 'validation_failed',
        detail: 'see errors',
        errors: [
          { field: 'code', code: 'format' },
          { field: 'code', code: 'length' },
          { field: 'quantity', code: 'out_of_range' },
          { field: 42, code: 'ignored' },
        ],
      }),
    );
    expect(problem).toBeInstanceOf(ApiProblem);
    expect(isApiProblem(problem)).toBe(true);
    expect(problem.status).toBe(400);
    expect(problem.code).toBe('validation_failed');
    expect(problem.title).toBe('The request contains invalid fields');
    expect(problem.detail).toBe('see errors');
    expect(problem.translationKey).toBe('errors.validation_failed');
    expect(problem.fieldErrors).toHaveLength(3);
    expect(problem.fieldTranslationKeys()).toEqual({
      code: 'errors.field.format',
      quantity: 'errors.field.out_of_range',
    });
  });

  it('reads a body left as text', () => {
    const body = JSON.stringify({ title: 'Resource not found', status: 404, code: 'not_found' });
    const problem = ApiProblem.fromHttpError(httpError(404, body));
    expect(problem.code).toBe('not_found');
    expect(problem.detail).toBeNull();
    expect(problem.fieldErrors).toEqual([]);
  });

  it('reports a request that never got a response as a network error', () => {
    const problem = ApiProblem.fromHttpError(httpError(0, new ProgressEvent('error'), 'Unknown Error'));
    expect(problem.status).toBe(0);
    expect(problem.code).toBe('network');
    expect(problem.translationKey).toBe('errors.network');
  });

  it('falls back on the status when the body is not Problem Details', () => {
    expect(ApiProblem.fromHttpError(httpError(502, '<html>Bad gateway</html>')).code).toBe('internal');
    expect(ApiProblem.fromHttpError(httpError(418, null)).code).toBe('unknown');
    expect(ApiProblem.fromHttpError(httpError(400, ['not', 'an', 'object'])).code).toBe('unknown');
  });

  it('treats a code missing from the catalog as unknown, keeping the status', () => {
    const problem = ApiProblem.fromHttpError(httpError(409, { code: 'future.code', title: 'Later' }));
    expect(problem.status).toBe(409);
    expect(problem.code).toBe('unknown');
    expect(problem.translationKey).toBe('errors.unknown');
  });

  it('is an Error with a readable message', () => {
    const problem = new ApiProblem(404, 'not_found', 'Resource not found');
    expect(problem).toBeInstanceOf(Error);
    expect(problem.name).toBe('ApiProblem');
    expect(problem.message).toBe('404 not_found: Resource not found');
  });
});
