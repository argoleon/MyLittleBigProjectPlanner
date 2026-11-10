import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { ActivatedRouteSnapshot, Router, RouterStateSnapshot, UrlTree, provideRouter } from '@angular/router';

import { ApiProblem } from '../api/api-problem';
import { provideApi } from '../api/provide-api';
import { ContractBackend, contractExample } from '../api/testing/contract-backend';
import { authGuard, signedOutGuard, stageGuard } from './auth.guards';
import { AuthService, isSafeReturnUrl, pathForStage } from './auth.service';

const UNAUTHENTICATED = { code: 'unauthenticated', title: 'Sign-in required, or the session has ended' };

function setUp() {
  TestBed.configureTestingModule({
    providers: [provideApi(), provideHttpClientTesting(), provideRouter([])],
  });
  const http = TestBed.inject(HttpTestingController);
  return {
    auth: TestBed.inject(AuthService),
    router: TestBed.inject(Router),
    http,
    backend: new ContractBackend(http),
  };
}

/** Lets pending promise callbacks run. */
const settle = () => new Promise((resolve) => setTimeout(resolve));

describe('AuthService', () => {
  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('asks the API for the session once, and remembers it', async () => {
    const { auth, backend, http } = setUp();
    expect(auth.session()).toBeUndefined();
    const first = auth.load();
    const second = auth.load();
    backend.respond(200);
    expect(await first).toEqual(contractExample('Session'));
    expect(await second).toEqual(contractExample('Session'));
    expect(auth.signedIn()).toBe(true);
    expect(await auth.load()).toEqual(contractExample('Session'));
    http.expectNone('/api/v1/auth/session');
  });

  it('knows it is signed out when the API answers 401', async () => {
    const { auth, backend } = setUp();
    const loaded = auth.load();
    backend.respond(401, UNAUTHENTICATED);
    expect(await loaded).toBeNull();
    expect(auth.session()).toBeNull();
    expect(await auth.load()).toBeNull();
  });

  it('asks again later when the API could not answer', async () => {
    const { auth, http, backend } = setUp();
    const loaded = auth.load();
    http.expectOne('/api/v1/auth/session').error(new ProgressEvent('error'));
    expect(await loaded).toBeNull();
    expect(auth.session()).toBeUndefined();
    const again = auth.load();
    backend.respond(200);
    expect(await again).not.toBeNull();
  });

  it('signs in with the email and password', async () => {
    const { auth, backend } = setUp();
    const signedIn = auth.signIn('lucas.bernard@acme.example', 'correct horse battery');
    const request = backend.respond(200);
    expect(request.request.body).toEqual({ email: 'lucas.bernard@acme.example', password: 'correct horse battery' });
    expect((await signedIn).account.displayName).toBe('Lucas Bernard');
    expect(auth.signedIn()).toBe(true);
  });

  it('reports wrong credentials as a problem and stays signed out', async () => {
    const { auth, backend } = setUp();
    const signedIn = auth.signIn('a@b.io', 'wrong').catch((e: unknown) => e);
    backend.respond(401, { code: 'auth.invalid_credentials', title: 'Incorrect email or password' });
    const error = (await signedIn) as ApiProblem;
    expect(error.translationKey).toBe('errors.auth.invalid_credentials');
    expect(auth.signedIn()).toBe(false);
  });

  it('signs out and shows the sign-in page, even when the API fails', async () => {
    const { auth, backend, router } = setUp();
    const navigate = vi.spyOn(router, 'navigateByUrl').mockResolvedValue(true);
    void auth.signIn('a@b.io', 'correct horse battery');
    backend.respond(200);
    await settle();

    const signedOut = auth.signOut();
    backend.respond(204);
    await signedOut;
    expect(auth.session()).toBeNull();
    expect(navigate).toHaveBeenCalledWith('/sign-in');

    const failed = auth.signOut();
    backend.respond(401, UNAUTHENTICATED);
    await failed;
    expect(navigate).toHaveBeenCalledTimes(2);
  });

  it('is signed in only once the session is complete', async () => {
    const { auth, backend } = setUp();
    const signedIn = auth.signIn('a@b.io', 'correct horse battery');
    backend.respond(200, { stage: 'mfa_challenge' });
    expect((await signedIn).stage).toBe('mfa_challenge');
    expect(auth.signedIn()).toBe(false);

    const verified = auth.verifySignIn('482913');
    const request = backend.respond(200);
    expect(request.request.url).toBe('/api/v1/auth/sign-in/verify');
    expect(request.request.body).toEqual({ code: '482913' });
    expect((await verified).stage).toBe('complete');
    expect(auth.signedIn()).toBe(true);
  });

  it('takes the session completed by an enrollment', async () => {
    const { auth, backend } = setUp();
    void auth.signIn('a@b.io', 'correct horse battery');
    backend.respond(200, { stage: 'mfa_enrollment' });
    await settle();
    const confirmed = auth.confirmMfa('123456');
    expect(backend.respond(200).request.body).toEqual({ code: '123456' });
    expect((await confirmed).backupCodes.length).toBeGreaterThan(0);
    expect(auth.session()?.stage).toBe('complete');
  });

  it('requests and confirms password resets', async () => {
    const { auth, backend } = setUp();
    const requested = auth.requestPasswordReset('lucas.bernard@acme.example');
    expect(backend.respond(202).request.body).toEqual({ email: 'lucas.bernard@acme.example' });
    await requested;

    const confirmed = auth.confirmPasswordReset('token-1', 'a brand new passphrase');
    expect(backend.respond(204).request.body).toEqual({ token: 'token-1', password: 'a brand new passphrase' });
    await confirmed;
    expect(auth.session()).toBeNull();
  });

  it('shows the sign-in page when the session ends, then comes back', async () => {
    const { auth, backend, router } = setUp();
    void auth.load();
    backend.respond(200);
    await settle();
    vi.spyOn(router, 'url', 'get').mockReturnValue('/gantt?view=week');
    const navigate = vi.spyOn(router, 'navigate').mockResolvedValue(true);

    auth.sessionEnded();
    expect(auth.session()).toBeNull();
    expect(navigate).toHaveBeenCalledWith(['/sign-in'], { queryParams: { returnUrl: '/gantt?view=week' } });

    // Already signed out: nothing more to do.
    auth.sessionEnded();
    expect(navigate).toHaveBeenCalledTimes(1);
  });
});

describe('pathForStage', () => {
  it('knows the page of each stage', () => {
    expect(pathForStage('mfa_challenge')).toBe('/sign-in/verify');
    expect(pathForStage('mfa_enrollment')).toBe('/sign-in/enroll');
    expect(pathForStage('complete')).toBeNull();
  });
});

describe('isSafeReturnUrl', () => {
  it('follows only return URLs of this application', () => {
    expect(isSafeReturnUrl('/gantt?view=week')).toBe(true);
    for (const url of ['//evil.example', '/\\evil.example', 'https://evil.example', 'gantt', '', null, 3]) {
      expect(isSafeReturnUrl(url), String(url)).toBe(false);
    }
  });
});

describe('route guards', () => {
  afterEach(() => TestBed.inject(HttpTestingController).verify());

  const route = {} as ActivatedRouteSnapshot;
  const state = (url: string) => ({ url }) as RouterStateSnapshot;
  const run = (guard: typeof authGuard, url: string) =>
    TestBed.runInInjectionContext(() => guard(route, state(url))) as Promise<boolean | UrlTree>;

  it('send signed-out visitors to the sign-in page, with the way back', async () => {
    const { backend, router } = setUp();
    const result = run(authGuard, '/workload');
    backend.respond(401, UNAUTHENTICATED);
    expect(router.serializeUrl((await result) as UrlTree)).toBe('/sign-in?returnUrl=%2Fworkload');
    expect(router.serializeUrl((await run(authGuard, '/')) as UrlTree)).toBe('/sign-in');
    expect(await run(signedOutGuard, '/sign-in')).toBe(true);
  });

  it('send a session waiting for its second factor to that step', async () => {
    const { backend, router } = setUp();
    const result = run(authGuard, '/gantt');
    backend.respond(200, { stage: 'mfa_challenge' });
    expect(router.serializeUrl((await result) as UrlTree)).toBe('/sign-in/verify?returnUrl=%2Fgantt');
    expect(await run(signedOutGuard, '/sign-in')).toBe(true);
    expect(await run(stageGuard('mfa_challenge'), '/sign-in/verify')).toBe(true);
    expect(router.serializeUrl((await run(stageGuard('mfa_enrollment'), '/sign-in/enroll')) as UrlTree)).toBe(
      '/sign-in',
    );
  });

  it('send a session that must enroll to the enrollment page', async () => {
    const { backend, router } = setUp();
    const result = run(authGuard, '/');
    backend.respond(200, { stage: 'mfa_enrollment' });
    expect(router.serializeUrl((await result) as UrlTree)).toBe('/sign-in/enroll');
  });

  it('let members in, and send them home from the sign-in page', async () => {
    const { backend, router } = setUp();
    const result = run(authGuard, '/workload');
    backend.respond(200);
    expect(await result).toBe(true);
    expect(router.serializeUrl((await run(signedOutGuard, '/sign-in')) as UrlTree)).toBe('/');
  });
});
