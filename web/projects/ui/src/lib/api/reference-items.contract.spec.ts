/**
 * Integration test of the generated client: the reference endpoint called
 * through `Api`, `provideApi` and its interceptor, against a backend mocked
 * from the OpenAPI contract (`testing/contract-backend.ts`).
 */
import { HttpRequest } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';

import { ApiProblem } from './api-problem';
import { Api } from './generated/api';
import {
  createReferenceItem,
  deleteReferenceItem,
  getReferenceItem,
  listReferenceItems,
  updateReferenceItem,
} from './generated/functions';
import type { ReferenceItemInput } from './generated/models/reference-item-input';
import { provideApi } from './provide-api';
import { ContractBackend, contract, contractExample, findOperation, schemaViolations } from './testing/contract-backend';

const ID = '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8b';
const INPUT: ReferenceItemInput = {
  code: 'REF-1',
  labelTranslations: { fr: 'Référence', en: 'Reference' },
  quantity: 3,
};

describe('generated client against the contract', () => {
  let api: Api;
  let http: HttpTestingController;
  let backend: ContractBackend;

  beforeEach(() => {
    TestBed.configureTestingModule({ providers: [provideApi(), provideHttpClientTesting()] });
    api = TestBed.inject(Api);
    http = TestBed.inject(HttpTestingController);
    backend = new ContractBackend(http);
  });

  afterEach(() => http.verify());

  it('has a function for every operation of the contract', () => {
    const functions = [
      createReferenceItem,
      deleteReferenceItem,
      getReferenceItem,
      listReferenceItems,
      updateReferenceItem,
    ];
    for (const fn of functions) {
      expect(Object.keys(contract.paths)).toContain(fn.PATH);
    }
  });

  it('creates a reference item idempotently', async () => {
    const created = api.invoke(createReferenceItem, {
      'Idempotency-Key': 'create-ref-1',
      'Accept-Language': 'en',
      body: INPUT,
    });
    const request = backend.respond(201);

    expect(request.request.method).toBe('POST');
    expect(request.request.url).toBe('/api/v1/reference-items');
    expect(request.request.headers.get('Idempotency-Key')).toBe('create-ref-1');
    expect(request.request.withCredentials).toBe(true);
    expect(request.request.body).toEqual(INPUT);
    expect(await created).toEqual(contractExample('ReferenceItem'));
  });

  it('reads one item and lists a page', async () => {
    const item = api.invoke(getReferenceItem, { id: ID, fields: 'id,code' });
    const read = backend.respond(200);
    expect(read.request.url).toBe(`/api/v1/reference-items/${ID}`);
    expect(read.request.params.get('fields')).toBe('id,code');
    expect((await item).code).toBe('REF-1');

    const page = api.invoke(listReferenceItems, { limit: 2 });
    backend.respond(200, { nextCursor: null });
    const body = await page;
    expect(body.items).toHaveLength(1);
    expect(body.nextCursor).toBeNull();
  });

  it('updates with If-Match and reports a stale version as a translated problem', async () => {
    const updated = api.invoke(updateReferenceItem, { id: ID, 'If-Match': '"1"', body: INPUT });
    const request = backend.respond(412, {
      type: 'urn:mlbpp:problem:precondition_failed',
      title: 'The resource was modified in the meantime',
      code: 'precondition_failed',
      errors: [],
    });
    expect(request.request.headers.get('If-Match')).toBe('"1"');

    const error = await updated.then(
      () => null,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(ApiProblem);
    expect((error as ApiProblem).status).toBe(412);
    expect((error as ApiProblem).translationKey).toBe('errors.precondition_failed');
  });

  it('deletes with If-Match', async () => {
    const deleted = api.invoke(deleteReferenceItem, { id: ID, 'If-Match': '*' });
    expect(backend.respond(204).request.method).toBe('DELETE');
    await deleted;
  });

  it('refuses responses the contract does not allow', () => {
    const update = () =>
      api.invoke(updateReferenceItem, { id: ID, 'If-Match': '"1"', body: INPUT }).catch(() => undefined);
    update();
    expect(() => backend.respond(418)).toThrow(/never answers 418/);
    update();
    expect(() => backend.respond(412, { code: 'not_a_code' })).toThrow(/not one of the allowed values/);
  });

  it('refuses requests the contract does not define', () => {
    const patch = new HttpRequest('PATCH', `/api/v1/reference-items/${ID}`, null);
    expect(() => findOperation(patch)).toThrow(/not in the contract/);
    expect(() => findOperation(new HttpRequest('GET', '/api/v1/unknown'))).toThrow(/not in the contract/);
    expect(
      schemaViolations({ $ref: '#/components/schemas/ReferenceItemInput' }, { code: 'X', extra: 1 }),
    ).toEqual(['$.labelTranslations: required', '$.extra: not in the schema']);
  });
});
