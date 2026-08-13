"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

function contentContext() {
  const windowListeners = new Map();
  const runtimeListeners = [];
  const window = {
    addEventListener(type, listener) { windowListeners.set(type, listener); },
    postMessage() {},
  };
  window.window = window;
  const context = vm.createContext({
    window,
    location: { href: "https://x.com/home", origin: "https://x.com" },
    document: {
      documentElement: null,
      addEventListener() {},
      querySelectorAll: () => [],
    },
    chrome: {
      runtime: {
        onMessage: { addListener(listener) { runtimeListeners.push(listener); } },
        sendMessage: async () => ({}),
      },
    },
    MutationObserver: class { observe() {} },
    URL,
    Date,
    Map,
    Set,
    Promise,
    setTimeout,
    clearTimeout,
  });
  const source = fs.readFileSync(path.resolve(__dirname, "..", "content.js"), "utf8");
  vm.runInContext(source, context);
  return context;
}

test("normalizes timeline JSON and preserves real top and bottom cursors", () => {
  const context = contentContext();
  const payload = {
    data: {
      timeline: {
        instructions: [{ entries: [
          {
            entryId: "tweet-42",
            content: { itemContent: { tweet_results: { result: {
              __typename: "Tweet",
              rest_id: "42",
              legacy: {
                full_text: "captured without rendering",
                created_at: "Wed Oct 10 20:19:24 +0000 2018",
                reply_count: 1,
                retweet_count: 2,
                favorite_count: 3,
              },
              core: { user_results: { result: {
                rest_id: "7",
                is_blue_verified: true,
                legacy: { name: "Reader", screen_name: "reader" },
              } } },
              views: { count: "99" },
            } } } },
          },
          { content: { cursorType: "Top", value: "top-token" } },
          { content: { cursorType: "Bottom", value: "bottom-token" } },
        ] }],
      },
    },
  };
  context.fixture = payload;
  const normalized = vm.runInContext("normalizeTimeline(fixture)", context);
  assert.equal(normalized.posts.length, 1);
  assert.equal(normalized.posts[0].id, "42");
  assert.equal(normalized.posts[0].username, "reader");
  assert.equal(normalized.posts[0].text, "captured without rendering");
  assert.equal(normalized.posts[0].views, 99);
  assert.match(normalized.posts[0].created_at, /^2018-10-10T20:19:24/);
  assert.equal(normalized.top_cursor, "top-token");
  assert.equal(normalized.bottom_cursor, "bottom-token");
});

test("head batches prepend new posts without moving existing posts backward", () => {
  const context = contentContext();
  context.fixture = {
    rest_id: "10",
    legacy: { full_text: "new", screen_name: "ignored" },
    core: { user_results: { result: { legacy: { name: "A", screen_name: "a" } } } },
  };
  const normalized = vm.runInContext("normalizeTimeline(fixture)", context);
  assert.deepEqual(Array.from(normalized.posts, (post) => post.id), ["10"]);
});

test("captured templates are scoped to the active XTUI route", () => {
  const context = contentContext();
  assert.equal(vm.runInContext('operationMatchesRoute("HomeLatestTimeline", "home")', context), true);
  assert.equal(vm.runInContext('operationMatchesRoute("TweetDetail", "home")', context), false);
  assert.equal(vm.runInContext('operationMatchesRoute("TweetDetail", "thread")', context), true);
  assert.equal(vm.runInContext('operationMatchesRoute("UserTweetsAndReplies", "user_posts")', context), true);
});
