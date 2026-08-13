"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

function post(id, username = "reader") {
  return {
    id: String(id),
    text: `post ${id}`,
    name: username,
    username,
    verified: false,
    created_at: null,
    replies: 0,
    reposts: 0,
    likes: 0,
    views: 0,
    media: [],
    quoted: null,
  };
}

function backgroundHarness() {
  let nextTabId = 1;
  const tabs = new Map();
  const homePosts = Array.from({ length: 180 }, (_, index) => post(index + 1));
  const threadPosts = Array.from({ length: 8 }, (_, index) => post(index ? 1000 + index : 42));
  const listener = () => ({ addListener() {} });
  const port = {
    onMessage: listener(),
    onDisconnect: listener(),
    postMessage() {},
  };
  const chrome = {
    runtime: {
      connectNative: () => port,
      onInstalled: listener(),
      onStartup: listener(),
      onMessage: listener(),
      getManifest: () => ({ version: "0.2.6" }),
      id: "test-extension",
    },
    alarms: { create() {}, onAlarm: listener() },
    tabs: {
      async create(options) {
        const tab = { id: nextTabId++, status: "complete", url: options.url };
        tabs.set(tab.id, tab);
        return tab;
      },
      async get(id) {
        if (!tabs.has(id)) throw new Error("missing tab");
        return tabs.get(id);
      },
      async update(id, options) {
        Object.assign(tabs.get(id), options);
        return tabs.get(id);
      },
      async remove(id) {
        tabs.delete(id);
      },
      async sendMessage(id, message) {
        const tab = tabs.get(id);
        if (!tab) throw new Error("missing tab");
        if (message.type === "posts") {
          return { ok: true, value: tab.url.includes("/status/") ? threadPosts : homePosts };
        }
        if (message.type === "me") {
          return { ok: true, value: { username: "reader" } };
        }
        return { ok: true, value: true };
      },
    },
  };
  const context = vm.createContext({ chrome, URLSearchParams, setTimeout, clearTimeout });
  const source = fs.readFileSync(path.resolve(__dirname, "..", "background.js"), "utf8");
  vm.runInContext(source, context);
  return { context, tabs };
}

test("opening a thread cannot navigate or cancel the home reservoir", async () => {
  const { context, tabs } = backgroundHarness();
  const first = await vm.runInContext(
    'collectPage({ op: "home", feed: "following", cursor: null }, routeKeyFor({ op: "home", feed: "following" }))',
    context,
  );
  assert.equal(first.items.length, 12);
  const homeTab = [...tabs.values()].find((tab) => tab.url === "https://x.com/home");
  assert.ok(homeTab);

  const thread = await vm.runInContext(
    'collectThread({ op: "thread", conversation_id: "42", author: "reader", reply_count: 7 }, routeKeyFor({ op: "thread", conversation_id: "42" }))',
    context,
  );
  assert.equal(thread[0].id, "42");
  assert.equal(thread.length, 8);
  assert.equal(tabs.get(homeTab.id).url, "https://x.com/home");
  assert.equal(tabs.size, 2, "home and thread retain independent transport tabs");

  const second = await vm.runInContext(
    `collectPage({ op: "home", feed: "following", cursor: ${JSON.stringify(first.next_token)} }, routeKeyFor({ op: "home", feed: "following" }))`,
    context,
  );
  assert.equal(second.items.length, 12);
  assert.notDeepEqual(
    second.items.map((item) => item.id),
    first.items.map((item) => item.id),
  );
  await vm.runInContext(
    "Promise.all([...routeCaches.values()].flatMap((cache) => [cache.harvesting, cache.threadHarvesting]).filter(Boolean))",
    context,
  );
  await vm.runInContext("closeTransports(true)", context);
});
