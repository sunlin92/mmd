import type { AppOpenIntent } from './openIntent';

export type { AppOpenIntent } from './openIntent';

export type OpenIntentSettlement =
  | { kind: 'accepted' }
  | { kind: 'cancelled' }
  | { kind: 'failed'; error: unknown };

export interface OpenIntentCoordinatorCallbacks {
  /** Called once when an intent becomes the sole active intent. */
  onActivate(intent: AppOpenIntent): void;
  /** Called once after an active intent has been accepted, cancelled, or failed. */
  onSettle(intent: AppOpenIntent, settlement: OpenIntentSettlement): void;
}

/**
 * Serializes open requests around document-save and modal decisions. It owns
 * no file access: callers use the active opaque ID to ask the backend to
 * authorize and resolve a request.
 */
export class OpenIntentCoordinator {
  private activeIntent: AppOpenIntent | null = null;
  private readonly pendingIntents: AppOpenIntent[] = [];
  private modalActive = false;
  private draining = false;
  private settling = false;

  constructor(private readonly callbacks: OpenIntentCoordinatorCallbacks) {}

  get active(): AppOpenIntent | null {
    return this.activeIntent;
  }

  get pending(): readonly AppOpenIntent[] {
    return this.pendingIntents;
  }

  get isModalActive(): boolean {
    return this.modalActive;
  }

  enqueue(intent: AppOpenIntent): boolean {
    if (this.contains(intent)) return false;

    this.pendingIntents.push(intent);
    this.drain();
    return true;
  }

  setModalActive(active: boolean): void {
    this.modalActive = active;
    if (!active) this.drain();
  }

  acceptActive(intentId: string): boolean {
    return this.settleActive(intentId, { kind: 'accepted' });
  }

  cancelActive(intentId: string): boolean {
    return this.settleActive(intentId, { kind: 'cancelled' });
  }

  failActive(intentId: string, error: unknown): boolean {
    return this.settleActive(intentId, { kind: 'failed', error });
  }

  private contains(intent: AppOpenIntent): boolean {
    return [this.activeIntent, ...this.pendingIntents].some((candidate) => {
      if (candidate == null) return false;
      return candidate.id === intent.id || this.hasSameTarget(candidate, intent);
    });
  }

  private hasSameTarget(left: AppOpenIntent, right: AppOpenIntent): boolean {
    if (left.origin === 'backend' || right.origin === 'backend') {
      return left.origin === 'backend'
        && right.origin === 'backend'
        && left.targetKind === right.targetKind
        && left.displayPath === right.displayPath;
    }
    const leftAction = left.action;
    const rightAction = right.action;
    if (leftAction.kind !== rightAction.kind) return false;
    switch (leftAction.kind) {
      case 'new_document':
      case 'open_file':
      case 'open_directory':
        return true;
      case 'open_recent':
        return rightAction.kind === leftAction.kind && leftAction.entryId === rightAction.entryId;
      case 'workspace_file':
        return rightAction.kind === leftAction.kind && leftAction.path === rightAction.path;
      case 'workspace_search_result':
        return rightAction.kind === leftAction.kind
          && leftAction.selection.workspaceToken === rightAction.selection.workspaceToken
          && leftAction.selection.workspaceRoot === rightAction.selection.workspaceRoot
          && leftAction.selection.indexGeneration === rightAction.selection.indexGeneration
          && leftAction.selection.relativePath === rightAction.selection.relativePath;
      case 'crash_draft':
        return rightAction.kind === leftAction.kind
          && leftAction.draft.documentId === rightAction.draft.documentId
          && leftAction.draft.entryToken === rightAction.draft.entryToken
          && leftAction.draft.draftRevision === rightAction.draft.draftRevision;
    }
  }

  private settleActive(intentId: string, settlement: OpenIntentSettlement): boolean {
    const active = this.activeIntent;
    if (active == null || active.id !== intentId || this.settling) return false;

    this.activeIntent = null;
    this.settling = true;
    try {
      this.callbacks.onSettle(active, settlement);
    } finally {
      this.settling = false;
    }
    this.drain();
    return true;
  }

  private drain(): void {
    if (this.draining || this.settling || this.modalActive || this.activeIntent != null) return;

    this.draining = true;
    try {
      const next = this.pendingIntents.shift();
      if (next == null || this.modalActive || this.activeIntent != null) return;

      this.activeIntent = next;
      this.callbacks.onActivate(next);
    } finally {
      this.draining = false;
    }
  }
}
