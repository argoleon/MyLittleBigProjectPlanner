/**
 * Unit tests of the account's preferences: applied to Transloco and to the
 * formatting of instants, with the API mocked at the HTTP level.
 */
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { TranslocoService } from '@jsverse/transloco';
import { InstantPipe, LocaleService, Me, provideApi } from 'ui';
import { provideI18nTesting } from 'ui/testing';

import { MyAccount } from './my-account';

const ME: Me = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01',
  email: 'lucas.bernard@acme.example',
  displayName: 'Lucas Bernard',
  locale: null,
  timeZone: null,
  hasPassword: true,
  tenantId: 't',
  tenantRole: 'member',
  accountType: 'internal',
};

const TENANT = { id: 't', slug: 'acme', name: 'Acme', defaultLocale: 'fr', timeZone: 'Europe/Paris' };

/** 2026-11-11 08:30 UTC. */
const INSTANT = '2026-11-11T08:30:00Z';

function setUp() {
  localStorage.clear();
  TestBed.configureTestingModule({ providers: [provideApi(), provideHttpClientTesting(), provideI18nTesting()] });
  const http = TestBed.inject(HttpTestingController);
  const account = TestBed.inject(MyAccount);
  const locale = TestBed.inject(LocaleService);
  const time = () =>
    TestBed.runInInjectionContext(() => new InstantPipe()).transform(INSTANT, { hour: '2-digit', minute: '2-digit' });
  const load = async (me: Me, tenant = TENANT) => {
    const loaded = account.load();
    await Promise.resolve();
    http.expectOne('/api/v1/me').flush(me);
    http.expectOne('/api/v1/tenant').flush(tenant);
    await loaded;
  };
  return { http, account, locale, time, load };
}

describe('MyAccount', () => {
  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('applies the account’s language to Transloco and its time zone to instants', async () => {
    const { locale, time, load } = setUp();
    expect(locale.language()).toBe('en');
    await load({ ...ME, locale: 'fr', timeZone: 'Asia/Tokyo' });
    expect(locale.language()).toBe('fr');
    expect(TestBed.inject(TranslocoService).getActiveLang()).toBe('fr');
    expect(TestBed.inject(TranslocoService).translate('account.title')).toBe('Mon compte');
    expect(locale.timeZone()).toBe('Asia/Tokyo');
    expect(time()).toBe('17:30');
  });

  it('keeps the device’s language and shows the organization’s time zone without a choice', async () => {
    const { locale, time, load } = setUp();
    await load(ME);
    expect(locale.language()).toBe('en');
    expect(locale.timeZone()).toBe('Europe/Paris');
    expect(time()).toBe('09:30 AM');
  });

  it('saves preferences, then applies them', async () => {
    const { http, account, locale, time, load } = setUp();
    await load(ME);
    const saved = account.save({ locale: 'fr', timeZone: 'America/New_York' });
    const request = http.expectOne('/api/v1/me/preferences');
    expect(request.request.method).toBe('PUT');
    expect(request.request.body).toEqual({ locale: 'fr', timeZone: 'America/New_York' });
    request.flush({ locale: 'fr', timeZone: 'America/New_York' });
    await saved;
    expect(account.preferences()).toEqual({ locale: 'fr', timeZone: 'America/New_York' });
    expect(locale.language()).toBe('fr');
    expect(time()).toBe('03:30');

    // Back to the organization's time zone.
    const reset = account.save({ locale: 'fr', timeZone: null });
    http.expectOne('/api/v1/me/preferences').flush({ locale: 'fr', timeZone: null });
    await reset;
    expect(locale.timeZone()).toBe('Europe/Paris');
  });

  it('loads once, and again after a failure', async () => {
    const { http, account } = setUp();
    const first = account.load();
    const second = account.load();
    expect(second).toBe(first);
    await Promise.resolve();
    http.expectOne('/api/v1/me').flush({ code: 'internal', status: 500 }, { status: 500, statusText: 'Error' });
    http.expectOne('/api/v1/tenant').flush(TENANT);
    await expect(first).rejects.toBeTruthy();
    expect(account.preferences()).toBeNull();
    const retried = account.load();
    expect(retried).not.toBe(first);
    await Promise.resolve();
    http.expectOne('/api/v1/me').flush(ME);
    http.expectOne('/api/v1/tenant').flush(TENANT);
    await retried;
    expect(account.preferences()).toEqual({ locale: null, timeZone: null });
  });
});
