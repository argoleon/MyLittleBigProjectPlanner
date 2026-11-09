# Adding a REST endpoint

Every endpoint of the API follows the same conventions (specification §15). The **reference resource** `/api/v1/reference-items` implements all of them on a trivial resource: copy it rather than starting from scratch.

| Layer | Reference file | Holds |
| --- | --- | --- |
| Rules | `backend/crates/domain/src/reference_item.rs` | Input struct and `validate`, no I/O |
| SQL | `backend/crates/infra/src/reference_items.rs` | Queries checked at compile time |
| HTTP | `backend/crates/api/src/reference_items.rs` | Handlers, request and response bodies |
| Table | `backend/migrations/*_idempotency_and_reference_item.sql` | Business table conventions |
| Tests | `backend/crates/api/tests/reference_items.rs` | One test per convention, on real PostgreSQL |
| Contract | `backend/openapi.json` | OpenAPI contract generated from the annotations |
| Client | `web/projects/ui/src/lib/api/generated/` | Angular client generated from the contract |

## Steps

Every command runs in the dev containers through `just` (nothing runs natively).

1. **Migration.** Add `backend/migrations/<timestamp>_<name>.sql`. Follow the checklist of `backend/migrations/README.md`: `id uuid`, `tenant_id`, `created_at`, `updated_at`, `version`, indexes starting with `tenant_id`, then `SELECT app_setup_business_table('<table>');`. Apply it with `just migrate`.
2. **Rules** in `crates/domain`: an input struct and a `validate` function returning every `FieldError` at once (`Violations`). Unit-test each rule. `crates/domain` must keep no dependency (`just check-domain`).
3. **SQL** in `crates/infra`: functions taking `&mut PgConnection` (the tenant transaction). Never filter on `tenant_id` and never pass it: row-level security scopes reads, and inserts use `app_current_tenant()`. Use `sqlx::query!` / `query_as!` with a private `Row` struct when a column needs a wrapper (`Json<…>`). Conditional writes take the expected versions (`version = ANY($n)`) and report `NotFound` or `VersionMismatch`.
4. **Refresh the query metadata** with `just sqlx-prepare` and commit `backend/.sqlx`. Builds and CI compile against it without a database.
5. **Handlers** in `crates/api`, registered in `routes.rs` with `routes!(…)` (one call per path):
   - take `TenantTx` first: it is the only database access, and the tenant is already set;
   - use `ApiJson`, `ApiPath` and `ApiQuery` instead of axum's extractors, so input errors are Problem Details;
   - return `Result<_, Problem>`. Add new error codes to `ErrorCode` in `problem.rs`: plain names for generic codes, `resource.reason` for specific ones. A code never changes once published.
   - call `tx.commit().await?` before answering. Dropping the transaction rolls back.
   - document each handler with `#[utoipa::path(…)]`, as the reference does: a unique camelCase `operation_id`, the resource's `tag`, a `summary`, every parameter and header (`If-Match`, `Idempotency-Key`…), and every status it can answer. Error statuses use `body = ProblemBody, content_type = PROBLEM_JSON`. Request and response structs derive `ToSchema` with an `example`; `///` comments on them become descriptions in the contract, so keep implementation notes in `//` comments.
6. **Contract and client.** Run `just generate-client`: it writes `backend/openapi.json`, then regenerates the Angular client. Commit both. A unit test fails if the contract is out of date, and `just client-check` (CI) fails with the diff if the client is. Check the result in the interactive documentation, `http://127.0.0.1:18080/api/v1/docs/`.
7. **Integration tests** in `crates/api/tests/`, with `testkit::TestDb` and the helpers of `tests/common`. Cover the happy path, validation errors, `If-Match`, pagination, and **access from another tenant (404)**. Unit and integration tests are mandatory.
8. `just check` and `just web-check` must pass before opening the pull request.

## Conventions, as implemented by the reference

| Convention | Request | Response |
| --- | --- | --- |
| Errors | — | `application/problem+json` with `type`, `title`, `status`, stable `code`, optional `detail` and `errors[{field, code}]`. 500s never reveal internals. |
| Validation | `POST` / `PUT` body | 400 `validation_failed` with every field error |
| Unknown JSON fields | `{"colour": …}` | 400 `invalid_json` (`deny_unknown_fields`) |
| Creation | `POST` | 201, `Location`, `ETag`, the resource |
| Idempotency | `Idempotency-Key: <uuid>` on `POST` | Replay: same status, body, `Location`, `ETag`, plus `Idempotent-Replayed: true`. Same key with another body: 422 `idempotency_key.reused`. Keys last 24 hours, per tenant. |
| Concurrency | `If-Match: "3"` on `PUT` / `DELETE` (required) | 412 `precondition_failed` if the version changed; 428 `precondition_required` without the header |
| Caching | `If-None-Match: "3"` on `GET` | 304 if unchanged |
| Pagination | `?limit=50&cursor=…` (limit 1 to 200) | `{"items": [...], "nextCursor": "…" or null}`, in creation order |
| Field selection | `?fields=id,code` | Only these fields; unknown name: 400 `invalid_query` |
| Languages | `Accept-Language: en` | Labels in that language, else the tenant's default; `Content-Language` header. Every translation with `?fields=…,labelTranslations` |
| Isolation | Host `<tenant>.example.org` | Another tenant's resource is a 404, never a 403: its existence is not disclosed |
| JSON names | camelCase | camelCase |

## Calling it from Angular

Applications call `provideApi()` (from the `ui` library) once, then use the generated `Api` service and functions; components never use `HttpClient` directly.

```ts
const api = inject(Api);
try {
  const page = await api.invoke(listReferenceItems, { limit: 20 });
} catch (error) {
  if (isApiProblem(error)) {
    // error.translationKey: "errors.<code>"; error.fieldTranslationKeys(): per invalid field
  }
}
```

Every failed call rejects with an `ApiProblem`: its `code` comes from the `ErrorCode` catalog of the contract, or is `network` / `unknown` for failures that are not Problem Details. Tests use `ContractBackend` (`web/projects/ui/src/lib/api/testing/`), a mocked backend that answers with the contract's examples and rejects requests the contract does not define.

## Trying it locally

The interactive documentation, with every endpoint and error code, is at `http://127.0.0.1:18080/api/v1/docs/` once `just dev` runs; the contract is at `/api/v1/openapi.json`.

```sh
just dev                 # API on http://127.0.0.1:18080
just seed                # creates the development tenant "default"
curl -i -X POST http://127.0.0.1:18080/api/v1/reference-items \
  -H 'content-type: application/json' -H "idempotency-key: $(uuidgen)" \
  -d '{"code": "REF-1", "labelTranslations": {"fr": "Référence", "en": "Reference"}, "quantity": 3}'
curl -i 'http://127.0.0.1:18080/api/v1/reference-items?limit=10' -H 'accept-language: en'
```
