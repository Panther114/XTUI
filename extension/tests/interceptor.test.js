"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

test("captured GET templates can be replayed with a replacement cursor", () => {
  const window = {
    fetch: async () => { throw new Error("not called"); },
    addEventListener() {},
    postMessage() {},
  };
  window.window = window;
  const context = vm.createContext({
    window,
    location: { href: "https://x.com/home", origin: "https://x.com" },
    URL,
    Headers,
    Request,
    structuredClone,
    Map,
    Date,
  });
  const source = fs.readFileSync(path.resolve(__dirname, "..", "interceptor.js"), "utf8");
  vm.runInContext(source, context);
  const variables = encodeURIComponent(JSON.stringify({ count: 20, cursor: "old" }));
  context.template = {
    url: `https://x.com/i/api/graphql/hash/HomeTimeline?variables=${variables}`,
    method: "GET",
    headers: {},
    body: null,
  };
  const replay = vm.runInContext('withCursor(template, "next")', context);
  const parsed = JSON.parse(new URL(replay.url).searchParams.get("variables"));
  assert.equal(parsed.cursor, "next");
  context.replay = replay;
  const head = vm.runInContext("withCursor(replay, null)", context);
  assert.equal("cursor" in JSON.parse(new URL(head.url).searchParams.get("variables")), false);
});
