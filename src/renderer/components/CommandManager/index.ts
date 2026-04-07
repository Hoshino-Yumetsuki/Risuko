import logger from "@shared/utils/logger";
import EventEmitter from "eventemitter3";

export default class CommandManager extends EventEmitter {
	private commands: Record<string, (...args: unknown[]) => unknown>;
	constructor() {
		super();

		this.commands = {};
	}

	register(id: string, fn: (...args: unknown[]) => unknown) {
		if (this.commands[id]) {
			logger.log(
				`[Motrix] Attempting to register an already-registered command: ${id}`,
			);
			return null;
		}
		if (!id || !fn) {
			logger.error(
				"[Motrix] Attempting to register a command with a missing id, or command function.",
			);
			return null;
		}
		this.commands[id] = fn;

		this.emit("commandRegistered", id);
	}

	unregister(id: string) {
		if (this.commands[id]) {
			delete this.commands[id];

			this.emit("commandUnregistered", id);
		}
	}

	execute(id: string, ...args: unknown[]) {
		const fn = this.commands[id];
		if (fn) {
			try {
				this.emit("beforeExecuteCommand", id);
			} catch (err) {
				logger.error(err);
			}
			const result = fn(...args);
			return result;
		} else {
			return false;
		}
	}
}
