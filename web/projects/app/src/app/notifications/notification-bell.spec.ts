/**
 * Tests of the bell of the top app bar (L05-US07), the API mocked at the
 * HTTP level and the real translations: the unread count as a badge and in
 * the label, read again after a navigation, kept when the API cannot
 * answer; accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { NotificationBell } from './notification-bell';

@Component({ template: '' })
class Blank {}

describe('the notification bell', () => {
  async function open() {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'elsewhere', component: Blank }]),
        provideApi(),
        provideHttpClientTesting(),
        provideI18nTesting(),
        provideMaterialDefaults(),
      ],
    });
    const http = TestBed.inject(HttpTestingController);
    const fixture = TestBed.createComponent(NotificationBell);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve));
        fixture.detectChanges();
      }
    };
    const reply = async (body: object, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match('/api/v1/me/notifications/unread');
        if (!request) await new Promise((resolve) => setTimeout(resolve, 10));
      }
      if (!request) throw new Error('no count asked');
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const root = fixture.nativeElement as HTMLElement;
    return { fixture, root, reply, settle };
  }

  it('shows the unread count, and reads it again after a navigation', async () => {
    const { root, reply, settle } = await open();
    await reply({ total: 0, byTenant: [] });
    const link = root.querySelector('a')!;
    expect(link.getAttribute('href')).toBe('/notifications');
    expect(link.getAttribute('aria-label')).toBe('Notifications');
    expect(root.querySelector('.mat-badge-hidden')).not.toBeNull();

    await TestBed.inject(Router).navigateByUrl('/elsewhere');
    await settle();
    await reply({ total: 120, byTenant: [{ tenantId: 't1', unread: 120 }] });
    expect(link.getAttribute('aria-label')).toBe('Notifications, 120 unread');
    expect(root.querySelector('.mat-badge-content')?.textContent?.trim()).toBe('99+');
    expect(await axeViolations(root)).toEqual([]);
  });

  it('keeps the last count when the API cannot answer', async () => {
    const { root, reply, settle } = await open();
    await reply({ total: 3, byTenant: [] });
    await TestBed.inject(Router).navigateByUrl('/elsewhere');
    await settle();
    await reply({ type: 'about:blank', title: 'Unavailable', status: 503, code: 'unavailable' }, 503);
    expect(root.querySelector('a')!.getAttribute('aria-label')).toBe('Notifications, 3 unread');
  });
});
