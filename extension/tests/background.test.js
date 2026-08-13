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
  const sessionStorage = {};
  const chrome = {
    runtime: {
      connectNative: () => port,
      onInstalled: listener(),
      onStartup: listener(),
      onMessage: listener(),
      getManifest: () => ({ version: "0.3.7" }),
      id: "test-extension",
    },
    alarms: { create() {}, onAlarm: listener() },
    cookies: {
      async get() {
        return { value: "test-csrf" };
      },
    },
    storage: {
      session: {
        async get(key) { return { [key]: sessionStorage[key] }; },
        async set(values) { Object.assign(sessionStorage, values); },
        async clear() {
          for (const key of Object.keys(sessionStorage)) delete sessionStorage[key];
        },
      },
    },
    windows: {
      async getAll() {
        return [{ id: 1, focused: true }];
      },
    },
    tabs: {
      async query() {
        return [];
      },
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
  const context = vm.createContext({
    chrome,
    URLSearchParams,
    setTimeout,
    clearTimeout,
    importScripts() {},
    XtuiTimeline: { normalizeTimeline: (value) => value },
  });
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

test("captured head batches prepend and tail batches append", () => {
  const { context } = backgroundHarness();
  vm.runInContext('testCache = cacheFor("capture-test")', context);
  context.initial = [post(2), post(3)];
  context.head = [post(1), post(2)];
  context.head[1].text = "updated post 2";
  context.tail = [post(3), post(4)];
  vm.runInContext('absorb(testCache, initial, "tail")', context);
  vm.runInContext('absorb(testCache, head, "head")', context);
  vm.runInContext('absorb(testCache, tail, "tail")', context);
  const ids = vm.runInContext("[...testCache.posts.keys()]", context);
  assert.deepEqual([...ids], ["1", "2", "3", "4"]);
  assert.equal(vm.runInContext('testCache.posts.get("2").text', context), "updated post 2");
});

test("bootstrap identity and Following reuse the same Home transport", async () => {
  const { context, tabs } = backgroundHarness();
  await vm.runInContext('handleNative({ id: 1, op: "me" })', context);
  await vm.runInContext(
    'collectPage({ op: "home", feed: "following", cursor: null }, routeKeyFor({ op: "home", feed: "following" }))',
    context,
  );
  assert.equal([...tabs.values()].filter((tab) => tab.url === "https://x.com/home").length, 1);
  await vm.runInContext("closeTransports(true)", context);
});

test("returning Home closes secondary tabs but retains their cached posts", async () => {
  const { context, tabs } = backgroundHarness();
  await vm.runInContext(
    'collectPage({ op: "home", feed: "following", cursor: null }, routeKeyFor({ op: "home", feed: "following" }))',
    context,
  );
  await vm.runInContext(
    'collectThread({ op: "thread", conversation_id: "42", author: "reader", reply_count: 7 }, routeKeyFor({ op: "thread", conversation_id: "42" }))',
    context,
  );
  assert.equal(tabs.size, 2);
  await vm.runInContext("releaseSecondaryTransports()", context);
  assert.equal(tabs.size, 1);
  assert.equal(vm.runInContext("routeCaches.size", context), 2);
  assert.equal(
    vm.runInContext(
      'routeCaches.get(routeKeyFor({ op: "thread", conversation_id: "42" })).posts.size',
      context,
    ),
    8,
  );
  await vm.runInContext("closeTransports(true)", context);
});

test("direct Home transport returns posts without creating a browser tab", async () => {
  const { context, tabs } = backgroundHarness();
  const state = {
    session: { user_id: "u1" },
    entities: { users: { entities: { u1: { name: "Reader", screen_name: "reader" } } } },
    featureSwitch: { feature_one: { value: true } },
  };
  const html = `__INITIAL_STATE__=${JSON.stringify(state)};window.__META_DATA__={};` +
    `73796:"shared~bundle.LoggedInMain~bundle.HomeTimeline",73796:"abcdef1"`;
  const chunk = 'queryId:"query",operationName:"HomeLatestTimeline",' +
    'operationType:"query",metadata:{featureSwitches:["feature_one"],fieldToggles:[]}}}';
  context.fetch = async (url) => {
    if (String(url).includes("HomeLatestTimeline")) {
      return { ok: true, status: 200, async json() { return { posts: context.homePosts, bottom_cursor: null }; } };
    }
    if (String(url).includes("abs.twimg.com")) {
      return { ok: true, status: 200, async text() { return chunk; } };
    }
    return { ok: true, status: 200, async text() { return html; } };
  };
  context.homePosts = Array.from({ length: 24 }, (_, index) => post(index + 1));

  const page = await vm.runInContext(
    'collectDirectHome({ op: "home", feed: "following", cursor: null }, routeKeyFor({ op: "home", feed: "following" }))',
    context,
  );
  assert.equal(page.items.length, 12);
  assert.equal(tabs.size, 0, "direct Home must never create a tab");
});

test("direct thread transport returns replies without creating a browser tab", async () => {
  const { context, tabs } = backgroundHarness();
  const state = {
    session: { user_id: "u1" },
    entities: { users: { entities: { u1: { name: "Reader", screen_name: "reader" } } } },
    featureSwitch: { feature_one: { value: true } },
  };
  const html = `__INITIAL_STATE__=${JSON.stringify(state)};window.__META_DATA__={};` +
    `73796:"shared~bundle.LoggedInMain~bundle.HomeTimeline",73796:"abcdef1",` +
    `https://abs.twimg.com/responsive-web/client-web/main.1234567.js`;
  const homeChunk = 'queryId:"home-query",operationName:"HomeLatestTimeline",' +
    'operationType:"query",metadata:{featureSwitches:[],fieldToggles:[]}}}';
  const mainChunk = 'queryId:"thread-query",operationName:"TweetDetail",' +
    'operationType:"query",metadata:{featureSwitches:["feature_one"],fieldToggles:[]}}}';
  context.fetch = async (url) => {
    const value = String(url);
    if (value.includes("/thread-query/TweetDetail")) {
      return { ok: true, status: 200, async json() { return { posts: context.threadPosts }; } };
    }
    if (value.includes("main.1234567.js")) {
      return { ok: true, status: 200, async text() { return mainChunk; } };
    }
    if (value.includes("shared~bundle")) {
      return { ok: true, status: 200, async text() { return homeChunk; } };
    }
    return { ok: true, status: 200, async text() { return html; } };
  };
  context.threadPosts = Array.from({ length: 8 }, (_, index) => post(index ? 1000 + index : 42));

  const thread = await vm.runInContext(
    'collectDirectThread({ op: "thread", conversation_id: "42" }, routeKeyFor({ op: "thread", conversation_id: "42" }))',
    context,
  );
  assert.equal(thread[0].id, "42");
  assert.equal(thread.length, 8);
  assert.equal(tabs.size, 0, "direct threads must never create a tab");
});
