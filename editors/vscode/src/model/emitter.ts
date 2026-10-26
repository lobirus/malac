// A minimal event emitter with disposable subscriptions, shaped like
// vscode.EventEmitter so the model stays free of the vscode module.

export interface Disposable {
  dispose(): void;
}

export class Emitter<T> {
  private listeners = new Set<(value: T) => void>();

  readonly event = (listener: (value: T) => void): Disposable => {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  };

  fire(value: T): void {
    for (const listener of [...this.listeners]) {
      try {
        listener(value);
      } catch (error) {
        console.error(error);
      }
    }
  }

  dispose(): void {
    this.listeners.clear();
  }
}
