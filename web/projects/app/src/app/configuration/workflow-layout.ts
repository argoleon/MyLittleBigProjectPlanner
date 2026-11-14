import { StatusCategory, Workflow, WorkflowCategory, WorkflowStatus, WorkflowTransition } from 'ui';

/** Size of a status box, and the gaps between them, in SVG units. */
export const NODE = { width: 140, height: 44, gapX: 72, gapY: 36, margin: 24 } as const;

/** A status placed in the graph. */
export interface PlacedStatus {
  readonly status: WorkflowStatus;
  readonly x: number;
  readonly y: number;
  /** The chip category it is drawn with. */
  readonly category: StatusCategory;
  /** Out of reach from the initial status, or only from any status: drawn below, dashed. */
  readonly apart: boolean;
}

/** A transition drawn between two placed statuses, or from any status. */
export interface PlacedTransition {
  readonly transition: WorkflowTransition;
  /** The path of its arrow. */
  readonly path: string;
  /** Where its name goes. */
  readonly labelX: number;
  readonly labelY: number;
  readonly fromAny: boolean;
}

/** The graph of a workflow: its size and what is drawn in it. */
export interface WorkflowLayout {
  readonly width: number;
  readonly height: number;
  readonly statuses: readonly PlacedStatus[];
  readonly transitions: readonly PlacedTransition[];
}

/** The chip category of an API category. */
export function chipCategory(category: WorkflowCategory): StatusCategory {
  return category === 'in_progress' ? 'progress' : category;
}

/**
 * Places the statuses of `workflow` in columns by their distance from the
 * initial status, following the transitions from one status to another
 * (mockup 11: To do, In progress, In review, Done). The statuses reached
 * only from any status, or not at all, go on a row below. Transitions
 * forward are straight arrows, backward ones curve under their row, and
 * those from any status come from above their target.
 */
export function layoutWorkflow(workflow: Pick<Workflow, 'statuses' | 'transitions'>): WorkflowLayout {
  const { statuses, transitions } = workflow;
  const column = new Map<string, number>();
  const initial = statuses.find((s) => s.isInitial);
  if (initial) {
    column.set(initial.id, 0);
    const queue = [initial.id];
    while (queue.length > 0) {
      const current = queue.shift()!;
      for (const t of transitions) {
        if (t.fromId === current && !column.has(t.toId)) {
          column.set(t.toId, column.get(current)! + 1);
          queue.push(t.toId);
        }
      }
    }
  }
  const rows = new Map<number, number>();
  const step = NODE.width + NODE.gapX;
  const rowStep = NODE.height + NODE.gapY;
  // From-any arrows come from above the first row.
  const top = NODE.margin + (transitions.some((t) => t.fromId === null) ? NODE.gapY : 0);
  const placedMain: PlacedStatus[] = [];
  for (const status of statuses) {
    const col = column.get(status.id);
    if (col === undefined) continue;
    const row = rows.get(col) ?? 0;
    rows.set(col, row + 1);
    placedMain.push({ status, x: NODE.margin + col * step, y: top + row * rowStep, category: chipCategory(status.category), apart: false });
  }
  const mainRows = Math.max(1, ...rows.values());
  const apartY = top + mainRows * rowStep + NODE.gapY;
  const apart: PlacedStatus[] = statuses
    .filter((s) => !column.has(s.id))
    .map((status, i) => ({ status, x: NODE.margin + i * step, y: apartY, category: chipCategory(status.category), apart: true }));
  const placed = [...placedMain, ...apart];
  const at = new Map(placed.map((p) => [p.status.id, p]));

  const drawn: PlacedTransition[] = [];
  for (const transition of transitions) {
    const to = at.get(transition.toId);
    if (!to) continue;
    if (transition.fromId === null) {
      // From any status: down onto its target.
      const x = to.x + NODE.width / 2;
      const y1 = to.y - NODE.gapY + 6;
      drawn.push({ transition, path: `M ${x} ${y1} L ${x} ${to.y}`, labelX: x + 6, labelY: y1 + 10, fromAny: true });
      continue;
    }
    const from = at.get(transition.fromId);
    if (!from) continue;
    if (to.x > from.x) {
      // Forward: from the right side to the left side.
      const x1 = from.x + NODE.width;
      const y1 = from.y + NODE.height / 2;
      const x2 = to.x;
      const y2 = to.y + NODE.height / 2;
      drawn.push({ transition, path: `M ${x1} ${y1} L ${x2} ${y2}`, labelX: (x1 + x2) / 2, labelY: Math.min(y1, y2) - 6, fromAny: false });
    } else {
      // Backward or within a column: under both boxes.
      const x1 = from.x + NODE.width / 2;
      const x2 = to.x + NODE.width / 2;
      const y1 = from.y + NODE.height;
      const y2 = to.y + NODE.height;
      const below = Math.max(y1, y2) + NODE.gapY / 2;
      drawn.push({
        transition,
        path: `M ${x1} ${y1} C ${x1} ${below} ${x2} ${below} ${x2} ${y2}`,
        labelX: (x1 + x2) / 2,
        labelY: below + 4,
        fromAny: false,
      });
    }
  }
  const width = Math.max(...placed.map((p) => p.x + NODE.width), NODE.width) + NODE.margin;
  const height = Math.max(...placed.map((p) => p.y + NODE.height), NODE.height) + NODE.margin + NODE.gapY / 2;
  return { width, height, statuses: placed, transitions: drawn };
}
