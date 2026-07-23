export type CommandListener = (...args: unknown[]) => unknown;

export default class CommandEmitter {
	private readonly listeners = new Map<string, Set<CommandListener>>();

	on(event: string, listener: CommandListener) {
		let listeners = this.listeners.get(event);
		if (!listeners) {
			listeners = new Set<CommandListener>();
			this.listeners.set(event, listeners);
		}

		listeners.add(listener);
		return this;
	}

	off(event: string, listener: CommandListener) {
		const listeners = this.listeners.get(event);
		if (!listeners) {
			return this;
		}

		listeners.delete(listener);
		if (listeners.size === 0) {
			this.listeners.delete(event);
		}
		return this;
	}

	emit(event: string, ...args: unknown[]) {
		const listeners = this.listeners.get(event);
		if (!listeners || listeners.size === 0) {
			return false;
		}

		// Keep the current dispatch stable when listeners change subscriptions.
		for (const listener of [...listeners]) {
			listener.apply(this, args);
		}
		return true;
	}
}
