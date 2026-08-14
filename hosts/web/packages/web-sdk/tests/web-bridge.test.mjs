import assert from "node:assert/strict";
import test from "node:test";

import { decode, encode } from "@msgpack/msgpack";

import { DisposableScope, WebBridgeClient, WebBridgeError } from "../dist/index.js";

class FakeWebSocket {
  binaryType = "arraybuffer";
  listeners = new Map();
  sent = [];
  closed = false;
  onSend = null;

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  send(bytes) {
    if (this.closed) throw new Error("socket closed");
    const message = decode(bytes);
    this.sent.push(message);
    this.onSend?.(message);
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    this.emit("close", {});
  }

  emit(type, event) {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }

  open(sessionId = "session") {
    this.emit("open", {});
    this.message({
      type: "hello_ack",
      protocol_version: "1.0.0",
      session: { session_id: sessionId, capabilities: [], safe_mode: false },
    });
  }

  message(payload) {
    const bytes = encode(payload);
    this.emit("message", {
      data: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
    });
  }
}

function harness(options = {}) {
  const sockets = [];
  const client = new WebBridgeClient("ws://example.test/ws", {
    helloTimeoutMs: 25,
    requestTimeoutMs: 25,
    reconnectBaseDelayMs: 1,
    reconnectMaxDelayMs: 1,
    ...options,
    webSocketFactory: () => {
      const socket = new FakeWebSocket();
      sockets.push(socket);
      return socket;
    },
  });
  return { client, sockets };
}

async function connect(h) {
  const connected = h.client.connect();
  assert.equal(h.sockets.length, 1);
  h.sockets.at(-1).open(`session-${h.sockets.length}`);
  await connected;
  return h.sockets.at(-1);
}

async function waitFor(predicate, timeoutMs = 100) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error("condition timed out");
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
}

async function rejectsWithCode(promise, code) {
  await assert.rejects(promise, (error) => {
    assert.ok(error instanceof WebBridgeError);
    assert.equal(error.code, code);
    return true;
  });
}

test("100 concurrent requests complete and the pending limit is enforced", async () => {
  const h = harness({ maxPending: 128 });
  const socket = await connect(h);
  socket.onSend = (message) => {
    if (message.type === "rpc") {
      assert.ok(message.id instanceof Uint8Array);
      assert.equal(message.id.length, 16);
      queueMicrotask(() => socket.message({ type: "rpc_result", id: message.id, result: message.params }));
    }
  };

  const results = await Promise.all(
    Array.from({ length: 100 }, (_, index) => h.client.read("test", "echo", index)),
  );
  assert.deepEqual(results, Array.from({ length: 100 }, (_, index) => index));
  assert.equal(h.client.pendingCount, 0);

  socket.onSend = null;
  const pending = Array.from({ length: 128 }, () => h.client.read("test", "wait"));
  await rejectsWithCode(h.client.read("test", "overflow"), "pending_limit");
  h.client.close();
  await Promise.all(pending.map((request) => rejectsWithCode(request, "closed")));
  assert.equal(h.client.pendingCount, 0);
});

test("hello, request and send failures use stable error codes", async () => {
  const hello = harness({ helloTimeoutMs: 5 });
  const helloPromise = hello.client.connect();
  hello.sockets[0].emit("open", {});
  await rejectsWithCode(helloPromise, "hello_timeout");
  hello.client.close();

  const request = harness({ requestTimeoutMs: 5 });
  const requestSocket = await connect(request);
  await rejectsWithCode(request.client.read("test", "slow"), "request_timeout");
  assert.equal(request.client.pendingCount, 0);

  requestSocket.onSend = (message) => {
    if (message.type === "rpc") throw new Error("write failed");
  };
  await rejectsWithCode(request.client.write("test", "fail"), "send_failed");
  assert.equal(request.client.pendingCount, 0);
  request.client.close();
});

test("disconnect distinguishes retryable reads from uncertain writes", async () => {
  const h = harness();
  const socket = await connect(h);
  const read = h.client.read("test", "read");
  const write = h.client.write("test", "write");
  socket.close();

  await rejectsWithCode(read, "disconnected");
  await rejectsWithCode(write, "delivery_unknown");
  assert.equal(h.client.pendingCount, 0);
  h.client.close();
});

test("stale generations cannot complete new requests and subscriptions are restored", async () => {
  const h = harness();
  const first = await connect(h);
  const subscription = h.client.subscribe("tasks", () => undefined, "tasks.read");
  const firstSubscribe = first.sent.find((message) => message.type === "subscribe");
  assert.ok(firstSubscribe);

  first.close();
  await waitFor(() => h.sockets.length === 2);
  const second = h.sockets[1];
  second.open("session-2");
  await waitFor(() => h.client.state === "open");
  const restored = second.sent.find((message) => message.type === "subscribe");
  assert.deepEqual(restored.subscription_id, firstSubscribe.subscription_id);

  let requestId;
  second.onSend = (message) => {
    if (message.type === "rpc") requestId = message.id;
  };
  const current = h.client.read("test", "current");
  assert.ok(requestId);
  first.message({ type: "rpc_result", id: requestId, result: "stale" });
  await new Promise((resolve) => setTimeout(resolve, 1));
  assert.equal(h.client.pendingCount, 1);
  second.message({ type: "rpc_result", id: requestId, result: "current" });
  assert.equal(await current, "current");

  subscription.dispose();
  h.client.close();
  assert.equal(h.client.pendingCount, 0);
});

test("disposable scope retries failed cleanup without repeating completed effects", () => {
  const scope = new DisposableScope();
  const disposed = [];
  let attempts = 0;
  scope.own({ dispose: () => disposed.push("first") });
  scope.own({
    dispose() {
      attempts += 1;
      if (attempts === 1) throw new Error("injected cleanup failure");
      disposed.push("second");
    },
  });

  assert.throws(() => scope.dispose(), AggregateError);
  assert.deepEqual(disposed, ["first"]);
  scope.dispose();
  scope.dispose();
  assert.equal(attempts, 2);
  assert.deepEqual(disposed, ["first", "second"]);
});
