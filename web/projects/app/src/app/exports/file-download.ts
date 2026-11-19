import { DOCUMENT } from '@angular/common';
import { Injectable, inject } from '@angular/core';

/**
 * Starts the download of a file the API serves at `path`: the browser
 * follows its redirect to the storage, which sends it as an attachment, so
 * the page stays where it is.
 */
@Injectable({ providedIn: 'root' })
export class FileDownload {
  private readonly document = inject(DOCUMENT);

  start(path: string): void {
    const link = this.document.createElement('a');
    link.href = path;
    link.rel = 'noopener';
    link.style.display = 'none';
    this.document.body.appendChild(link);
    link.click();
    link.remove();
  }
}
