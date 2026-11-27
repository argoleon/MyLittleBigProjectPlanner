import { HttpErrorResponse } from '@angular/common/http';

import type { ErrorCode } from './generated/models/error-code';
import { ERROR_CODE } from './generated/models/error-code-array';
import type { FieldError } from './generated/models/field-error';
import type { OverloadProblem } from './generated/models/overload-problem';
import type { QueryProblem } from './generated/models/query-problem';

/**
 * Codes added on the client side, for failures that never reached the API
 * or did not come back as Problem Details (a proxy error page, for example).
 */
export type ClientErrorCode = 'network' | 'unknown';

/** Every code an {@link ApiProblem} can carry. */
export type ApiErrorCode = ErrorCode | ClientErrorCode;

/** Prefix of the translation keys of error codes: `errors.<code>`. */
export const ERROR_KEY_PREFIX = 'errors.';

/** Prefix of the translation keys of field error codes: `errors.field.<code>`. */
export const FIELD_ERROR_KEY_PREFIX = 'errors.field.';

const KNOWN_CODES: ReadonlySet<string> = new Set(ERROR_CODE);

/** Whether `code` belongs to the API catalog (the `ErrorCode` schema). */
export function isErrorCode(code: unknown): code is ErrorCode {
  return typeof code === 'string' && KNOWN_CODES.has(code);
}

/**
 * Translation key of an error code, e.g. `errors.not_found` or
 * `errors.reference_item.code_taken`. Codes outside the catalog map to
 * `errors.unknown`, so the interface always has a message to show.
 */
export function errorTranslationKey(code: string): string {
  const known = isErrorCode(code) || code === 'network' || code === 'unknown';
  return ERROR_KEY_PREFIX + (known ? code : 'unknown');
}

/** Translation key of a field error code, e.g. `errors.field.required`. */
export function fieldErrorTranslationKey(code: string): string {
  return FIELD_ERROR_KEY_PREFIX + code;
}

/**
 * An API error, parsed from an RFC 9457 Problem Details response.
 *
 * The generated client rejects with this class (see `provideApi`):
 * components show `translationKey` and never parse HTTP errors themselves.
 */
export class ApiProblem extends Error {
  override readonly name = 'ApiProblem';

  constructor(
    /** HTTP status; 0 when the request never got a response. */
    readonly status: number,
    readonly code: ApiErrorCode,
    /** English title sent by the API, for logs. Do not show it: translate `code`. */
    readonly title: string,
    readonly detail: string | null = null,
    readonly fieldErrors: readonly FieldError[] = [],
    /** For `query.invalid`: where and why the query was refused. */
    readonly query: QueryProblem | null = null,
    /** For an overload refused (code workload.overload): the days, and whether the member may force it. */
    readonly overload: OverloadProblem | null = null,
  ) {
    super(`${status} ${code}: ${title}`);
  }

  /** Translation key of the error as a whole. */
  get translationKey(): string {
    return errorTranslationKey(this.code);
  }

  /** Translation keys of the invalid fields, by field path. */
  fieldTranslationKeys(): Record<string, string> {
    const keys: Record<string, string> = {};
    for (const error of this.fieldErrors) {
      keys[error.field] ??= fieldErrorTranslationKey(error.code);
    }
    return keys;
  }

  /** Builds the problem of a failed HTTP call. */
  static fromHttpError(error: HttpErrorResponse): ApiProblem {
    if (error.status === 0) {
      return new ApiProblem(0, 'network', 'The server could not be reached');
    }
    const body = parseBody(error.error);
    if (body !== null && isErrorCode(body['code'])) {
      return new ApiProblem(
        error.status,
        body['code'],
        typeof body['title'] === 'string' ? body['title'] : error.statusText,
        typeof body['detail'] === 'string' ? body['detail'] : null,
        parseFieldErrors(body['errors']),
        parseQueryProblem(body['query']),
        parseOverloadProblem(body['overload']),
      );
    }
    // Not a Problem Details body, or a code this client does not know yet.
    const code: ApiErrorCode = error.status >= 500 ? 'internal' : 'unknown';
    return new ApiProblem(error.status, code, error.statusText || 'Unexpected error');
  }
}

/** Whether `error` is an {@link ApiProblem}. */
export function isApiProblem(error: unknown): error is ApiProblem {
  return error instanceof ApiProblem;
}

/** The error body as an object: HttpClient gives it parsed, or as text when parsing failed. */
function parseBody(body: unknown): Record<string, unknown> | null {
  if (typeof body === 'string') {
    try {
      body = JSON.parse(body);
    } catch {
      return null;
    }
  }
  return typeof body === 'object' && body !== null && !Array.isArray(body)
    ? (body as Record<string, unknown>)
    : null;
}

function parseQueryProblem(query: unknown): QueryProblem | null {
  if (typeof query !== 'object' || query === null || Array.isArray(query)) return null;
  const candidate = query as QueryProblem;
  return typeof candidate.reason === 'string' &&
    typeof candidate.position === 'number' &&
    typeof candidate.length === 'number'
    ? candidate
    : null;
}

function parseOverloadProblem(overload: unknown): OverloadProblem | null {
  if (typeof overload !== 'object' || overload === null || Array.isArray(overload)) return null;
  const candidate = overload as OverloadProblem;
  return Array.isArray(candidate.days) && typeof candidate.forceable === 'boolean' ? candidate : null;
}

function parseFieldErrors(errors: unknown): FieldError[] {
  if (!Array.isArray(errors)) {
    return [];
  }
  return errors.filter(
    (e): e is FieldError =>
      typeof e === 'object' &&
      e !== null &&
      typeof (e as FieldError).field === 'string' &&
      typeof (e as FieldError).code === 'string',
  );
}
