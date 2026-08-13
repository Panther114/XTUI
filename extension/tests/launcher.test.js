"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

test("the npm launcher gives XTUI exclusive synchronous console ownership", () => {
  const launcher = fs.readFileSync(
    path.resolve(__dirname, "..", "..", "bin", "xtui.js"),
    "utf8"
  );
  assert.match(launcher, /spawnSync\(/);
  assert.match(launcher, /stdio:\s*"inherit"/);
  assert.doesNotMatch(launcher, /\.on\("exit"/);
});
