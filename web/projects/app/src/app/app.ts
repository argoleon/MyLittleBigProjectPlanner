import { Component } from '@angular/core';
import { RouterOutlet } from '@angular/router';

/** Root of the internal application; its pages are listed in app.routes.ts. */
@Component({
  selector: 'app-root',
  imports: [RouterOutlet],
  template: `<router-outlet />`,
})
export class App {}
