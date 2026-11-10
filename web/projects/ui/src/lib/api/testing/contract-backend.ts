/**
 * A mocked HTTP backend generated from the OpenAPI contract, for tests.
 *
 * It answers requests made through `HttpTestingController` with the examples
 * of the contract, after checking that the request matches an operation of
 * the contract (method, path, required parameters and headers, body). The
 * responses are checked against their schema too, so a test cannot rely on
 * a response the API would never send.
 *
 * Test-only: not exported by the library.
 */
import { HttpRequest } from '@angular/common/http';
import { HttpTestingController, TestRequest } from '@angular/common/http/testing';

import contractJson from '../../../../../../../backend/openapi.json';

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

interface Schema {
  $ref?: string;
  type?: string | string[];
  properties?: Record<string, Schema>;
  required?: string[];
  items?: Schema;
  additionalProperties?: Schema | boolean;
  enum?: Json[];
  example?: Json;
}

interface Parameter {
  name: string;
  in: 'query' | 'header' | 'path';
  required?: boolean;
}

interface MediaType {
  schema: Schema;
}

interface Operation {
  operationId: string;
  parameters?: Parameter[];
  requestBody?: { required?: boolean; content: Record<string, MediaType> };
  responses: Record<string, { content?: Record<string, MediaType> }>;
}

interface Contract {
  paths: Record<string, Record<string, Operation>>;
  components: { schemas: Record<string, Schema> };
}

/** The contract the API serves (`backend/openapi.json`). */
export const contract = contractJson as unknown as Contract;

const HTTP_METHODS = ['get', 'put', 'post', 'delete', 'patch', 'head', 'options'];

/** The operation of the contract matching `request`; throws if there is none. */
export function findOperation(request: HttpRequest<unknown>): Operation & { path: string } {
  const method = request.method.toLowerCase();
  const url = new URL(request.url, 'http://localhost');
  for (const [path, item] of Object.entries(contract.paths)) {
    const pattern = new RegExp(`^${path.replace(/\{[^}]+\}/g, '[^/]+')}$`);
    const operation = HTTP_METHODS.includes(method) ? item[method] : undefined;
    if (operation && pattern.test(url.pathname)) {
      return { ...operation, path };
    }
  }
  throw new Error(`${request.method} ${url.pathname} is not in the contract`);
}

/** Resolves `$ref` against the contract components. */
function resolve(schema: Schema): Schema {
  if (schema.$ref === undefined) {
    return schema;
  }
  const name = schema.$ref.replace('#/components/schemas/', '');
  const target = contract.components.schemas[name];
  if (!target) {
    throw new Error(`unknown schema ${schema.$ref}`);
  }
  return resolve(target);
}

/** The example of a component schema, e.g. `contractExample('ReferenceItem')`. */
export function contractExample(name: string): Json {
  return exampleOf({ $ref: `#/components/schemas/${name}` });
}

/** An example value for `schema`: its own example, else one built from its parts. */
export function exampleOf(schema: Schema): Json {
  const resolved = resolve(schema);
  if (resolved.example !== undefined) {
    return structuredClone(resolved.example);
  }
  if (resolved.enum?.length) {
    return resolved.enum[0];
  }
  const types = typesOf(resolved);
  if (types.includes('object') || resolved.properties) {
    const value: Record<string, Json> = {};
    for (const [key, property] of Object.entries(resolved.properties ?? {})) {
      value[key] = exampleOf(property);
    }
    return value;
  }
  if (types.includes('array')) {
    return resolved.items ? [exampleOf(resolved.items)] : [];
  }
  if (types.includes('string')) {
    return 'string';
  }
  if (types.includes('integer') || types.includes('number')) {
    return 0;
  }
  if (types.includes('boolean')) {
    return false;
  }
  return null;
}

function typesOf(schema: Schema): string[] {
  if (schema.type === undefined) {
    return [];
  }
  return Array.isArray(schema.type) ? schema.type : [schema.type];
}

function typeOf(value: Json): string {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  if (typeof value === 'number') return Number.isInteger(value) ? 'integer' : 'number';
  return typeof value;
}

/**
 * Checks `value` against `schema` (types, required properties, enums, items);
 * returns the problems found, as `path: message`.
 */
export function schemaViolations(schema: Schema, value: Json, path = '$'): string[] {
  const resolved = resolve(schema);
  const types = typesOf(resolved);
  const actual = typeOf(value);
  const typeMatches =
    types.length === 0 ||
    types.includes(actual) ||
    (actual === 'integer' && types.includes('number'));
  if (!typeMatches) {
    return [`${path}: expected ${types.join(' or ')}, got ${actual}`];
  }
  if (resolved.enum && !resolved.enum.includes(value)) {
    return [`${path}: ${JSON.stringify(value)} is not one of the allowed values`];
  }
  const problems: string[] = [];
  if (actual === 'object' && value !== null && !Array.isArray(value) && typeof value === 'object') {
    for (const key of resolved.required ?? []) {
      if (!(key in value)) {
        problems.push(`${path}.${key}: required`);
      }
    }
    for (const [key, item] of Object.entries(value)) {
      const property = resolved.properties?.[key];
      if (property) {
        problems.push(...schemaViolations(property, item, `${path}.${key}`));
      } else if (typeof resolved.additionalProperties === 'object') {
        problems.push(...schemaViolations(resolved.additionalProperties, item, `${path}.${key}`));
      } else if (resolved.properties && resolved.additionalProperties !== true) {
        problems.push(`${path}.${key}: not in the schema`);
      }
    }
  }
  if (Array.isArray(value) && resolved.items) {
    value.forEach((item, i) => problems.push(...schemaViolations(resolved.items!, item, `${path}[${i}]`)));
  }
  return problems;
}

function checkRequest(operation: Operation, request: HttpRequest<unknown>): void {
  const problems: string[] = [];
  for (const parameter of operation.parameters ?? []) {
    if (!parameter.required) continue;
    const present =
      parameter.in === 'query'
        ? request.params.has(parameter.name)
        : parameter.in === 'header'
          ? request.headers.has(parameter.name)
          : true; // path parameters are part of the matched path
    if (!present) {
      problems.push(`required ${parameter.in} parameter ${parameter.name} is missing`);
    }
  }
  const body = operation.requestBody;
  if (body) {
    if (request.body === null || request.body === undefined) {
      if (body.required) problems.push('the request body is missing');
    } else {
      const media = body.content['application/json'];
      if (media) problems.push(...schemaViolations(media.schema, request.body as Json, 'body'));
    }
  } else if (request.body !== null && request.body !== undefined) {
    problems.push('this operation takes no body');
  }
  if (problems.length > 0) {
    throw new Error(`${operation.operationId}: ${problems.join('; ')}`);
  }
}

/** Answers API requests from the contract. */
export class ContractBackend {
  constructor(private readonly http: HttpTestingController) {}

  /**
   * Answers the one pending request with the contract's example for
   * `status`, after checking the request. `overrides` replace fields of the
   * example (e.g. the `code` of a Problem); the result must still match the
   * response schema.
   */
  respond(status: number, overrides: Record<string, Json> = {}): TestRequest {
    const pending = this.http.match(() => true);
    if (pending.length !== 1) {
      throw new Error(`expected one pending request, found ${pending.length}`);
    }
    const [testRequest] = pending;
    const operation = findOperation(testRequest.request);
    checkRequest(operation, testRequest.request);

    const response = operation.responses[String(status)];
    if (!response) {
      throw new Error(`${operation.operationId} never answers ${status}`);
    }
    const [contentType, media] = Object.entries(response.content ?? {})[0] ?? [];
    let body: Json = null;
    if (media) {
      const example = exampleOf(media.schema);
      body =
        typeof example === 'object' && example !== null && !Array.isArray(example)
          ? { ...example, ...overrides }
          : example;
      if (contentType === 'application/problem+json' && typeof body === 'object' && body !== null) {
        (body as Record<string, Json>)['status'] = status;
      }
      const problems = schemaViolations(media.schema, body);
      if (problems.length > 0) {
        throw new Error(`${operation.operationId} ${status}: ${problems.join('; ')}`);
      }
    }
    testRequest.flush(body, {
      status,
      statusText: String(status),
      headers: contentType ? { 'content-type': contentType } : {},
    });
    return testRequest;
  }
}
