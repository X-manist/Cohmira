import { type GooseBridgeTaskSnapshot, type GooseBridgeTaskStatus } from './types.ts';

export class GooseBridgeTaskCancelledError extends Error {
  readonly code = 'GOOSE_BRIDGE_TASK_CANCELLED';

  constructor(taskId: string) {
    super(`Goose bridge task cancelled: ${taskId}`);
    this.name = 'GooseBridgeTaskCancelledError';
  }
}

type TaskHandler<T> = (signal: AbortSignal) => Promise<T> | T;

interface QueueItem<T> {
  snapshot: GooseBridgeTaskSnapshot;
  handler: TaskHandler<T>;
  controller: AbortController;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
}

export class GooseBridgeTaskQueue {
  private queue: Array<QueueItem<unknown>> = [];
  private active: QueueItem<unknown> | null = null;
  private history = new Map<string, GooseBridgeTaskSnapshot>();

  enqueue<T>(id: string, handler: TaskHandler<T>): Promise<T> {
    if (!id.trim()) {
      return Promise.reject(new Error('task id is required'));
    }
    if (this.history.has(id)) {
      return Promise.reject(new Error(`task already exists: ${id}`));
    }

    const snapshot: GooseBridgeTaskSnapshot = {
      id,
      status: 'queued',
      enqueuedAt: Date.now(),
    };
    const item = {
      snapshot,
      handler: handler as TaskHandler<unknown>,
      controller: new AbortController(),
      resolve: undefined as unknown as (value: unknown) => void,
      reject: undefined as unknown as (error: unknown) => void,
    };
    const promise = new Promise<T>((resolve, reject) => {
      item.resolve = resolve as (value: unknown) => void;
      item.reject = reject;
    });

    this.history.set(id, snapshot);
    this.queue.push(item);
    this.pump();
    return promise;
  }

  cancel(id: string): boolean {
    const queuedIndex = this.queue.findIndex((item) => item.snapshot.id === id);
    if (queuedIndex >= 0) {
      const [item] = this.queue.splice(queuedIndex, 1);
      this.finish(item, 'cancelled');
      item.reject(new GooseBridgeTaskCancelledError(id));
      return true;
    }

    if (this.active?.snapshot.id === id) {
      this.active.snapshot.status = 'canceling';
      this.active.controller.abort();
      return true;
    }

    return false;
  }

  snapshot(): GooseBridgeTaskSnapshot[] {
    return Array.from(this.history.values())
      .map((item) => ({ ...item }))
      .sort((left, right) => left.enqueuedAt - right.enqueuedAt);
  }

  get(id: string): GooseBridgeTaskSnapshot | undefined {
    const snapshot = this.history.get(id);
    return snapshot ? { ...snapshot } : undefined;
  }

  private pump(): void {
    if (this.active || this.queue.length === 0) return;
    const item = this.queue.shift()!;
    this.active = item;
    item.snapshot.status = 'running';
    item.snapshot.startedAt = Date.now();

    Promise.resolve()
      .then(() => item.handler(item.controller.signal))
      .then((value) => {
        if (item.controller.signal.aborted) {
          this.finish(item, 'cancelled');
          item.reject(new GooseBridgeTaskCancelledError(item.snapshot.id));
          return;
        }
        this.finish(item, 'completed');
        item.resolve(value);
      })
      .catch((error) => {
        const status: GooseBridgeTaskStatus = item.controller.signal.aborted ? 'cancelled' : 'failed';
        this.finish(item, status, error);
        item.reject(status === 'cancelled' ? new GooseBridgeTaskCancelledError(item.snapshot.id) : error);
      })
      .finally(() => {
        if (this.active === item) {
          this.active = null;
        }
        this.pump();
      });
  }

  private finish(item: QueueItem<unknown>, status: GooseBridgeTaskStatus, error?: unknown): void {
    item.snapshot.status = status;
    item.snapshot.finishedAt = Date.now();
    if (error) {
      item.snapshot.error = error instanceof Error ? error.message : String(error);
    }
  }
}
