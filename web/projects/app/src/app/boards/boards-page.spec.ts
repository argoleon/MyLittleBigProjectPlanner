/**
 * Tests of the boards list (L06-US05), the API mocked at the HTTP level and
 * the real translations: the boards by node, each opening its board; none
 * yet; a failed load; a new board opening once created; accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { Router, provideRouter } from '@angular/router';
import { Board, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';
import { of } from 'rxjs';

import { BoardSettingsDialog } from './board-settings-dialog';
import { BoardsPage } from './boards-page';

function board(id: string, name: string, node: { id: string; key: string; name: string }): Board {
  return {
    id,
    node: { ...node, nodeType: 'project', archived: false },
    name,
    kind: 'kanban',
    filter: `project = ${node.key}`,
    estimation: 'points',
    swimlanes: 'none',
    quickFilters: [],
    columns: [],
    canManage: false,
    version: 1,
  };
}

const PLAT = { id: 'p1', key: 'PLAT', name: 'Identity platform' };
const OPS = { id: 'p2', key: 'OPS', name: 'Operations' };

describe('the boards list', () => {
  async function open(items: readonly Board[] | null, node?: string) {
    TestBed.configureTestingModule({
      providers: [provideRouter([]), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
    });
    const http = TestBed.inject(HttpTestingController);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve, 20));
        TestBed.tick();
      }
    };
    const fixture = TestBed.createComponent(BoardsPage);
    if (node) fixture.componentRef.setInput('node', node);
    document.body.appendChild(fixture.nativeElement);
    await settle();
    let request: TestRequest | undefined;
    for (let i = 0; i < 50 && !request; i++) {
      [request] = http.match((r) => r.url === '/api/v1/boards');
      if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    if (node) {
      expect(request!.request.params.get('nodeId')).toBe(node);
      const [asked] = http.match((r) => r.url === `/api/v1/nodes/${node}`);
      asked.flush({ id: node, type: 'project', key: 'PLAT', name: 'Identity platform', archived: false });
    }
    if (items) request!.flush({ items });
    else request!.flush({ code: 'internal', status: 500, title: 'Error' }, { status: 500, statusText: 'Error' });
    await settle();
    const page = () => fixture.nativeElement as HTMLElement;
    const texts = (selector: string) => Array.from(page().querySelectorAll(selector)).map((e) => e.textContent?.replace(/\s+/g, ' ').trim());
    return { page, texts, settle };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists the boards by node, each opening its board', async () => {
    const { page, texts } = await open([board('b1', 'Platform board', PLAT), board('b2', 'Support', PLAT), board('b3', 'Run', OPS)]);
    expect(texts('h2.node')).toEqual(['Identity platform PLAT', 'Operations OPS']);
    expect(texts('.board [matListItemTitle]')).toEqual(['Platform board', 'Support', 'Run']);
    expect(texts('.board .filter')).toEqual(['project = PLAT', 'project = PLAT', 'project = OPS']);
    expect(page().querySelector('a.board')?.getAttribute('href')).toBe('/boards/b1');
    expect(await axeViolations(page())).toEqual([]);
  });

  it('says when there is no board yet', async () => {
    const { page } = await open([]);
    expect(page().querySelector('ui-empty-state')?.textContent).toContain('No board yet');
  });

  it('tells when the boards cannot be read', async () => {
    const { page } = await open(null);
    expect(page().querySelector('ui-error-state')).not.toBeNull();
  });

  it('shows the boards of one node, and creates new ones on it', async () => {
    const { page, texts, settle } = await open([board('b1', 'Platform board', PLAT)], 'p1');
    expect(texts('.subtitle')).toEqual(['Boards of Identity platform · All boards']);
    expect(page().querySelector('.subtitle a')?.getAttribute('href')).toBe('/boards');
    const dialog = TestBed.inject(MatDialog);
    const data: unknown[] = [];
    vi.spyOn(dialog, 'open').mockImplementation(((_: unknown, config: { data: unknown }) => {
      data.push(config.data);
      return { afterClosed: () => of(undefined) };
    }) as unknown as typeof dialog.open);
    (page().querySelector('button.new-board') as HTMLElement).click();
    await settle();
    expect(data).toEqual([{ nodeId: 'p1' }]);
  });

  it('opens the new board once created', async () => {
    const { page, settle } = await open([]);
    const created = board('b9', 'New one', PLAT);
    const dialog = TestBed.inject(MatDialog);
    const opened: unknown[] = [];
    vi.spyOn(dialog, 'open').mockImplementation(((component: unknown) => {
      opened.push(component);
      return { afterClosed: () => of(created) };
    }) as unknown as typeof dialog.open);
    const navigate = vi.spyOn(TestBed.inject(Router), 'navigate').mockResolvedValue(true);
    (page().querySelector('button.new-board') as HTMLElement).click();
    await settle();
    expect(opened).toEqual([BoardSettingsDialog]);
    expect(navigate).toHaveBeenCalledWith(['/boards', 'b9']);
  });
});
