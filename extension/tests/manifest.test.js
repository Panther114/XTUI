"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const manifest = JSON.parse(fs.readFileSync(path.join(root, "manifest.json"), "utf8"));
const packageManifest = JSON.parse(
  fs.readFileSync(path.join(root, "..", "package.json"), "utf8"),
);

test("extension version matches the CLI package", () => {
  assert.equal(manifest.version, packageManifest.version);
});

test("extension permissions stay narrowly scoped", () => {
  assert.equal(manifest.manifest_version, 3);
  assert.deepEqual(manifest.host_permissions, ["https://x.com/*"]);
  assert.deepEqual(manifest.permissions.sort(), ["alarms", "nativeMessaging", "tabs"]);
  assert.ok(!JSON.stringify(manifest).includes("cookies"));
  assert.ok(!JSON.stringify(manifest).includes("webRequest"));
});

test("native bridge and transport lifecycle are present", () => {
  const background = fs.readFileSync(path.join(root, "background.js"), "utf8");
  assert.match(background, /connectNative\(HOST\)/);
  assert.match(background, /active: false/);
  assert.match(background, /30000/);
  assert.match(background, /PREFETCH_RESERVOIR = 144/);
  assert.match(background, /pageFromCache\(cache, priorSeen, targetUnseen\)/);
  assert.match(background, /session: null/);
  assert.match(background, /startThreadHarvest/);
  assert.match(background, /temporary empty sample must never terminate/);
  assert.match(background, /Do not await the full harvest/);
  assert.doesNotMatch(background, /let currentRoute/);
  assert.match(background, /collectThread/);
});

test("content extraction accumulates virtualized posts and expands replies", () => {
  const content = fs.readFileSync(path.join(root, "content.js"), "utf8");
  assert.match(content, /new MutationObserver/);
  assert.match(content, /postCache\.set/);
  assert.match(content, /MAX_POST_CACHE = 800/);
  assert.match(content, /expand_thread/);
  assert.match(content, /tweet-text-show-more-link/);
  assert.match(content, /expand_text/);
  assert.match(content, /aggressive \? 2\.8 : 1\.45/);
});
