/**
 * Unit tests of the sentences of the Activity tab (L05-US06), with the real
 * translations, in English and French: field changes with their values
 * formatted in the locale (status labels, priorities, planning dates,
 * instants, numbers, yes/no, people no longer members), values set and
 * cleared, rich texts on demand, moves, links, comments, the system and
 * former members as authors.
 */
import { TestBed } from '@angular/core/testing';
import { TranslocoService } from '@jsverse/transloco';
import { ActivityEntry, ActivityValue, LocaleService } from 'ui';
import { provideI18nTesting } from 'ui/testing';

import { SentenceContext, sentenceOf } from './activity-sentence';

const value = (kind: ActivityValue['kind'], text: string | null, html: string | null = null): ActivityValue => ({ kind, text, html });

const entry = (rest: Partial<ActivityEntry>): ActivityEntry => ({
  id: 'e1',
  changeId: 'c1',
  action: 'updated',
  field: null,
  fieldLabel: null,
  relation: null,
  old: null,
  new: null,
  actor: { id: 'a1', name: 'Lucas Bernard' },
  origin: 'interface',
  createdAt: '2026-11-06T08:12:00Z',
  ...rest,
});

describe('sentences of the Activity tab', () => {
  function context(language: 'en' | 'fr'): SentenceContext {
    localStorage.clear();
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    TestBed.inject(LocaleService).use(language);
    const transloco = TestBed.inject(TranslocoService);
    return {
      translate: (key, params) => transloco.translate(key, params),
      locale: language,
      timeZone: 'Europe/Paris',
    };
  }

  // `use('fr')` remembers the language in the browser: forget it, for the specs that follow.
  afterEach(() => localStorage.clear());

  const say = (e: ActivityEntry, c: SentenceContext) => {
    const sentence = sentenceOf(e, c);
    return `${sentence.who} ${sentence.what}`;
  };

  it('says a status change, in English and in French', () => {
    const status = entry({ field: 'status', old: value('text', 'To do'), new: value('text', 'In progress') });
    expect(say(status, context('en'))).toBe('Lucas Bernard changed the status: To do → In progress');
    TestBed.resetTestingModule();
    const french = entry({ field: 'status', old: value('text', 'À faire'), new: value('text', 'En cours') });
    expect(say(french, context('fr'))).toBe('Lucas Bernard a changé le statut : À faire → En cours');
  });

  it('formats values in the locale', () => {
    const en = context('en');
    expect(say(entry({ field: 'priority', old: value('priority', 'medium'), new: value('priority', 'highest') }), en)).toBe(
      'Lucas Bernard changed the priority: Medium → Highest',
    );
    expect(say(entry({ field: 'dueDate', old: value('date', '2026-11-30'), new: value('date', '2026-12-04') }), en)).toBe(
      'Lucas Bernard changed the due date: Nov 30, 2026 → Dec 4, 2026',
    );
    expect(say(entry({ field: 'assignee', old: value('person', null), new: value('person', 'Camille Martin') }), en)).toBe(
      'Lucas Bernard changed the assignee: a former member → Camille Martin',
    );
    expect(
      say(entry({ field: 'custom', fieldLabel: 'Estimate', old: value('number', '1500.5'), new: value('number', '8') }), en),
    ).toBe('Lucas Bernard changed Estimate: 1,500.5 → 8');
    expect(say(entry({ field: 'custom', fieldLabel: 'Billable', old: value('boolean', 'false'), new: value('boolean', 'true') }), en)).toBe(
      'Lucas Bernard changed Billable: no → yes',
    );
    expect(
      say(entry({ field: 'custom', fieldLabel: 'Go-live', old: null, new: value('dateTime', '2026-11-06T08:12:00Z') }), en),
    ).toBe('Lucas Bernard set Go-live to Nov 6, 2026, 9:12 AM');
    expect(say(entry({ field: 'custom', fieldLabel: null, old: value('text', 'x'), new: null }), en)).toBe(
      'Lucas Bernard cleared a removed field (was x)',
    );
    TestBed.resetTestingModule();
    const fr = context('fr');
    expect(say(entry({ field: 'dueDate', old: null, new: value('date', '2026-11-30') }), fr)).toBe(
      'Lucas Bernard a renseigné l’échéance : 30 nov. 2026',
    );
    expect(say(entry({ field: 'priority', old: value('priority', 'medium'), new: value('priority', 'high') }), fr)).toBe(
      'Lucas Bernard a changé la priorité : Moyenne → Haute',
    );
  });

  it('keeps rich texts for later, and says moves, links and comments', () => {
    const en = context('en');
    const description = sentenceOf(
      entry({ field: 'description', old: value('richText', null, '<p>Before</p>'), new: value('richText', null, '<p>After</p>') }),
      en,
    );
    expect(description.what).toBe('changed the description');
    expect(description.change).toEqual({ before: '<p>Before</p>', after: '<p>After</p>' });
    expect(say(entry({ action: 'created' }), en)).toBe('Lucas Bernard created the ticket');
    expect(say(entry({ action: 'moved', old: value('issue', 'PLAT-2'), new: value('issue', 'CRM-1') }), en)).toBe(
      'Lucas Bernard moved the ticket: PLAT-2 → CRM-1',
    );
    expect(say(entry({ action: 'link_added', relation: 'blocked_by', new: value('issue', 'PLAT-1') }), en)).toBe(
      'Lucas Bernard added a link: Blocked by PLAT-1',
    );
    expect(say(entry({ action: 'link_removed', relation: 'relates_to', old: value('issue', null) }), en)).toBe(
      'Lucas Bernard removed a link: Relates to a deleted ticket',
    );
    expect(say(entry({ action: 'comment_created' }), en)).toBe('Lucas Bernard commented');
    expect(say(entry({ action: 'comment_updated' }), en)).toBe('Lucas Bernard edited a comment');
    const deleted = sentenceOf(entry({ action: 'comment_deleted', old: value('richText', null, '<p>Mine</p>') }), en);
    expect([deleted.what, deleted.deleted]).toEqual(['deleted a comment', '<p>Mine</p>']);
    expect(sentenceOf(entry({ action: 'comment_deleted' }), en).deleted).toBeNull();
  });

  it('says files attached and removed', () => {
    const en = context('en');
    expect(say(entry({ action: 'attachment_added', new: value('text', 'plan.pdf') }), en)).toBe('Lucas Bernard attached plan.pdf');
    expect(say(entry({ action: 'attachment_removed', old: value('text', 'plan.pdf') }), en)).toBe(
      'Lucas Bernard removed the file plan.pdf',
    );
    TestBed.resetTestingModule();
    expect(say(entry({ action: 'attachment_added', new: value('text', 'plan.pdf') }), context('fr'))).toBe('Lucas Bernard a joint plan.pdf');
  });

  it('says an overload forced, in English and in French', () => {
    const forced = entry({ action: 'overload_forced', new: value('text', 'Nathan Roux') });
    expect(say(forced, context('en'))).toBe('Lucas Bernard forced an overload of Nathan Roux');
    TestBed.resetTestingModule();
    expect(say(forced, context('fr'))).toBe('Lucas Bernard a forcé une surcharge de Nathan Roux');
  });

  it('names the system and former members', () => {
    const en = context('en');
    expect(sentenceOf(entry({ action: 'created', actor: null, origin: 'system' }), en).who).toBe('The system');
    expect(sentenceOf(entry({ action: 'created', actor: null }), en).who).toBe('A former member');
    expect(sentenceOf(entry({ action: 'created', actor: { id: 'a9', name: 'Deleted user #12' } }), en).who).toBe('Deleted user #12');
    TestBed.resetTestingModule();
    expect(sentenceOf(entry({ action: 'created', actor: null, origin: 'system' }), context('fr')).who).toBe('Le système');
  });
});
