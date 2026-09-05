import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";

/**
 * Structural guarantee, not just a comment: the main package entry must
 * never statically reach the module that sends the store's Meta access
 * token (`events/capi.ts`, published only through the `/meta-capi`
 * subpath). This reads the *compiled* output so a future re-export added to
 * `index.ts` or `ssr/server.ts` fails this test instead of silently
 * reopening the isolation gap described in `events/capi.ts`'s module doc.
 */
test("the main entry never statically imports the Meta CAPI sender", async () => {
  const distDir = fileURLToPath(new URL("..", import.meta.url));
  for (const file of ["index.js", "ssr/server.js"]) {
    const source = await readFile(`${distDir}${file}`, "utf8");
    assert.doesNotMatch(
      source,
      /events\/(capi|server)\.js/,
      `${file} must not import events/capi.js or events/server.js`,
    );
  }
});
