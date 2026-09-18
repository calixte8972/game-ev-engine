import assert from "node:assert/strict";

import worker from "../dist/server/index.js";

const assetUrl = "https://example.test/styles.css";
const first = await worker.fetch(new Request(assetUrl));
assert.equal(first.status, 200);
assert.match(first.headers.get("content-type") ?? "", /^text\/css/);
assert.equal(first.headers.get("cache-control"), "no-cache");

const etag = first.headers.get("etag");
assert.ok(etag, "静态资源必须携带基于内容的 ETag");
assert.ok((await first.arrayBuffer()).byteLength > 0, "首次请求必须返回资源内容");

const conditional = await worker.fetch(new Request(assetUrl, {
  headers: { "If-None-Match": etag },
}));
assert.equal(conditional.status, 304);
assert.equal((await conditional.arrayBuffer()).byteLength, 0);
assert.equal(conditional.headers.get("etag"), etag);

const head = await worker.fetch(new Request(assetUrl, { method: "HEAD" }));
assert.equal(head.status, 200);
assert.equal((await head.arrayBuffer()).byteLength, 0);
assert.equal(head.headers.get("etag"), etag);

const exportModule = await worker.fetch(new Request("https://example.test/replay-export.mjs"));
assert.equal(exportModule.status, 200);
assert.match(exportModule.headers.get("content-type") ?? "", /^text\/javascript/);

assert.equal((await worker.fetch(new Request("https://example.test/missing"))).status, 404);
assert.equal((await worker.fetch(new Request(assetUrl, { method: "POST" }))).status, 405);

console.log("PASS: Sites Worker ETag, conditional GET, HEAD, 404 and 405 responses");
