import assert from "node:assert/strict";
import test from "node:test";

test("the public package entry rejects a server runtime", async () => {
  Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "document");

  await assert.rejects(import("../index.js"), /browser-only/);
});
