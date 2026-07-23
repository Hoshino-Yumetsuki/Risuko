import assert from "node:assert/strict";
import { test } from "node:test";
import CommandEmitter from "./CommandEmitter.ts";

test("emit returns false when an event has no listeners", () => {
	const emitter = new CommandEmitter();

	assert.equal(emitter.emit("missing"), false);
});

test("listeners run synchronously in registration order with the emitter context", () => {
	const emitter = new CommandEmitter();
	const calls: string[] = [];
	const first = function (value: string) {
		assert.strictEqual(this, emitter);
		calls.push(`first:${value}`);
	};
	const second = function (value: string) {
		assert.strictEqual(this, emitter);
		calls.push(`second:${value}`);
	};

	assert.strictEqual(emitter.on("event", first), emitter);
	assert.strictEqual(emitter.on("event", second), emitter);
	assert.equal(emitter.emit("event", "payload"), true);
	assert.deepEqual(calls, ["first:payload", "second:payload"]);
});

test("off removes the exact listener and remains fluent", () => {
	const emitter = new CommandEmitter();
	let removedCalls = 0;
	let retainedCalls = 0;
	const removed = () => {
		removedCalls += 1;
	};
	const retained = () => {
		retainedCalls += 1;
	};

	emitter.on("event", removed).on("event", retained);
	assert.strictEqual(emitter.off("event", removed), emitter);
	assert.equal(emitter.emit("event"), true);
	assert.equal(removedCalls, 0);
	assert.equal(retainedCalls, 1);

	assert.strictEqual(emitter.off("event", retained), emitter);
	assert.equal(emitter.emit("event"), false);
});

test("subscription mutations do not alter an in-progress dispatch", () => {
	const emitter = new CommandEmitter();
	const calls: string[] = [];
	const added = () => {
		calls.push("added");
	};
	const second = () => {
		calls.push("second");
	};
	const first = () => {
		calls.push("first");
		emitter.off("event", second);
		emitter.on("event", added);
	};

	emitter.on("event", first).on("event", second);
	emitter.emit("event");
	assert.deepEqual(calls, ["first", "second"]);

	calls.length = 0;
	emitter.emit("event");
	assert.deepEqual(calls, ["first", "added"]);
});

test("listener errors propagate without invoking later listeners", () => {
	const emitter = new CommandEmitter();
	let laterListenerRan = false;

	emitter.on("event", () => {
		throw new Error("listener failure");
	});
	emitter.on("event", () => {
		laterListenerRan = true;
	});

	assert.throws(() => emitter.emit("event"), /listener failure/);
	assert.equal(laterListenerRan, false);
});
