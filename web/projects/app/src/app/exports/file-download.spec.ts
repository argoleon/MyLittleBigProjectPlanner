import { TestBed } from '@angular/core/testing';

import { FileDownload } from './file-download';

describe('FileDownload', () => {
  it('follows a link to the file, then removes it', () => {
    const clicked: string[] = [];
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
      clicked.push(this.getAttribute('href') ?? '');
      expect(this.isConnected).toBe(true);
    });
    TestBed.inject(FileDownload).start('/api/v1/exports/e1/file');
    expect(clicked).toEqual(['/api/v1/exports/e1/file']);
    expect(document.querySelector('a[href="/api/v1/exports/e1/file"]')).toBeNull();
    click.mockRestore();
  });
});
