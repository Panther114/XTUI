"use strict";

const HOST = "com.xtui.bridge";
const PAGE_SIZE = 12;
const HARVEST_BATCH = 48;
const PREFETCH_RESERVOIR = 144;
const MAX_CACHED_POSTS = 800;
const MAX_SCROLL_ROUNDS = 160;
const FIRST_POST_TIMEOUT_MS = 3500;
const NEXT_PAGE_WAIT_MS = 1400;

let nativePort = null;
let detachTimer = null;
const routeCaches = new Map();

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

function connectHost() {
  if (nativePort) return;
  try {
    nativePort = chrome.runtime.connectNative(HOST);
    nativePort.onMessage.addListener((request) => void handleNative(request));
    nativePort.onDisconnect.addListener(() => {
      nativePort = null;
      void closeTransports(true);
      setTimeout(connectHost, 1000);
    });
  } catch (_error) {
    nativePort = null;
  }
}

chrome.runtime.onInstalled.addListener(connectHost);
chrome.runtime.onStartup.addListener(connectHost);
chrome.alarms.create("xtui-native-reconnect", { periodInMinutes: 1 });
chrome.alarms.onAlarm.addListener(connectHost);
connectHost();

chrome.runtime.onMessage.addListener((message, _sender, reply) => {
  if (message?.type !== "popup-status") return false;
  void tabExists().then((tab_open) =>
    reply({
      connected: Boolean(nativePort),
      tab_open,
      extension_version: chrome.runtime.getManifest().version,
    })
  );
  return true;
});

function respond(id, result, error = null) {
  nativePort?.postMessage(error ? { id, ok: false, error } : { id, ok: true, result });
}

async function tabExists() {
  const sessions = [...routeCaches.values()].map((cache) => cache.session).filter(Boolean);
  if (!sessions.length) return false;
  const states = await Promise.all(sessions.map((session) => sessionExists(session)));
  return states.some(Boolean);
}

function routeFor(request) {
  const cleanUser = String(request.user_id || "").replace(/^@/, "");
  switch (request.op) {
    case "me":
    case "home":
      return "https://x.com/home";
    case "search": {
      const query = new URLSearchParams({ q: request.query || "", src: "typed_query", f: "live" });
      return `https://x.com/search?${query}`;
    }
    case "bookmarks":
      return "https://x.com/i/bookmarks";
    case "mentions":
      return "https://x.com/notifications/mentions";
    case "user":
    case "user_posts":
      return `https://x.com/${cleanUser}`;
    case "likes":
      return `https://x.com/${cleanUser}/likes`;
    case "lists":
      return `https://x.com/${cleanUser}/lists`;
    case "list_posts":
      return `https://x.com/i/lists/${encodeURIComponent(request.list_id || "")}`;
    case "thread":
      return request.author
        ? `https://x.com/${String(request.author).replace(/^@/, "")}/status/${encodeURIComponent(request.conversation_id || "")}`
        : `https://x.com/i/web/status/${encodeURIComponent(request.conversation_id || "")}`;
    default:
      throw new Error(`unsupported operation: ${request.op}`);
  }
}

function routeKeyFor(request) {
  return JSON.stringify([
    request.op,
    request.query || "",
    request.user_id || "",
    request.list_id || "",
    request.conversation_id || "",
    request.feed || "",
  ]);
}

function cacheFor(routeKey) {
  let cache = routeCaches.get(routeKey);
  if (!cache) {
    cache = {
      posts: new Map(),
      harvesting: null,
      threadHarvesting: null,
      revision: 0,
      touched: Date.now(),
      session: null,
    };
    routeCaches.set(routeKey, cache);
  }
  cache.touched = Date.now();
  if (routeCaches.size > 12) {
    const oldest = [...routeCaches.entries()]
      .filter(([key]) => key !== routeKey)
      .sort((a, b) => a[1].touched - b[1].touched)[0];
    if (oldest) {
      routeCaches.delete(oldest[0]);
      void closeSession(oldest[1].session);
    }
  }
  return cache;
}

function absorb(cache, posts) {
  let added = 0;
  for (const post of posts || []) {
    if (!post?.id) continue;
    if (!cache.posts.has(post.id)) {
      cache.revision += 1;
      added += 1;
    }
    cache.posts.set(post.id, post);
    while (cache.posts.size > MAX_CACHED_POSTS) {
      cache.posts.delete(cache.posts.keys().next().value);
    }
  }
  return added;
}

function parseCursor(cursor) {
  if (!cursor) return [];
  try {
    const parsed = JSON.parse(cursor);
    return Array.isArray(parsed.seen) ? parsed.seen.slice(-1000) : [];
  } catch (_error) {
    return [];
  }
}

function pageFromCache(cache, priorSeen, limit = PAGE_SIZE) {
  const seen = new Set(priorSeen);
  return [...cache.posts.values()].filter((post) => !seen.has(post.id)).slice(0, limit);
}

function makePage(cache, priorSeen) {
  const items = pageFromCache(cache, priorSeen);
  const seen = [...priorSeen, ...items.map((post) => post.id)].slice(-1000);
  return {
    items,
    // X timelines are open-ended. Even an idle DOM can receive another batch
    // after a network pause, so a temporary empty sample must never terminate
    // pagination permanently.
    next_token: JSON.stringify({ seen, revision: cache.revision }),
  };
}

async function waitForComplete(tabId) {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    const tab = await chrome.tabs.get(tabId);
    if (tab.status === "complete") return;
    await delay(50);
  }
  throw new Error("X did not finish loading");
}

async function sendContent(tabId, message, attempts = 30) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const response = await chrome.tabs.sendMessage(tabId, message);
      if (response?.ok) return response.value;
      if (response?.error) throw new Error(response.error);
    } catch (error) {
      if (attempt === attempts - 1) throw error;
    }
    await delay(75);
  }
  throw new Error("XTUI content bridge did not become ready");
}

async function sessionExists(session) {
  if (!session?.tabId) return false;
  try {
    await chrome.tabs.get(session.tabId);
    return true;
  } catch (_error) {
    session.tabId = null;
    session.generation += 1;
    return false;
  }
}

async function closeSession(session) {
  if (!session?.tabId) return;
  try {
    await chrome.tabs.remove(session.tabId);
  } catch (_error) {}
  session.tabId = null;
  session.generation += 1;
}

async function ensureRoute(url, cache) {
  clearTimeout(detachTimer);
  if (!cache.session) cache.session = { tabId: null, url, generation: 0 };
  const session = cache.session;
  const exists = await sessionExists(session);
  if (!exists) {
    const tab = await chrome.tabs.create({ active: false, url });
    session.tabId = tab.id;
    session.url = url;
    session.generation += 1;
    await chrome.tabs.update(session.tabId, { muted: true });
    await waitForComplete(session.tabId);
    return { ...session };
  }
  return { ...session };
}

async function waitForPosts(cache, priorSeen, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (pageFromCache(cache, priorSeen, 1).length) return;
    if (!cache.harvesting) return;
    await delay(50);
  }
}

function startHarvest(request, cache, priorSeen) {
  if (cache.harvesting) return cache.harvesting;
  cache.harvesting = (async () => {
    const url = routeFor(request);
    const session = await ensureRoute(url, cache);
    const targetUnseen = Math.min(
      PREFETCH_RESERVOIR,
      Math.max(HARVEST_BATCH, MAX_CACHED_POSTS - priorSeen.length),
    );
    if (request.op === "home") {
      await sendContent(session.tabId, { type: "feed", feed: request.feed || "following" }, 12);
    }
    let idleRounds = 0;
    for (let round = 0; round < MAX_SCROLL_ROUNDS; round += 1) {
      if (!cache.session || session.generation !== cache.session.generation) return;
      await sendContent(session.tabId, { type: "expand_text" }, 3);
      const added = absorb(cache, await sendContent(session.tabId, { type: "posts" }, 8));
      idleRounds = added ? 0 : idleRounds + 1;
      if (pageFromCache(cache, priorSeen, targetUnseen).length >= targetUnseen) {
        return;
      }
      await sendContent(session.tabId, { type: "scroll", aggressive: true }, 5);
      // Adaptive backoff: stay quick while cards arrive, give X more time only
      // when its virtualized timeline is genuinely waiting on the network.
      await delay(added ? 70 : Math.min(140 + idleRounds * 100, 1000));
    }
  })().finally(() => {
    cache.harvesting = null;
  });
  return cache.harvesting;
}

async function collectPage(request, routeKey) {
  const cache = cacheFor(routeKey);
  const priorSeen = parseCursor(request.cursor);

  // Cached cards are returned without navigating the hidden browser. This is
  // what keeps Back, search revisits, and fast scrolling effectively instant.
  if (pageFromCache(cache, priorSeen, 1).length) {
    startHarvest(request, cache, priorSeen);
    return makePage(cache, priorSeen);
  }

  const harvest = startHarvest(request, cache, priorSeen);
  await waitForPosts(
    cache,
    priorSeen,
    request.cursor ? NEXT_PAGE_WAIT_MS : FIRST_POST_TIMEOUT_MS
  );
  // Do not await the full harvest. It deliberately continues behind the TUI.
  void harvest;
  return makePage(cache, priorSeen);
}

function startThreadHarvest(request, cache) {
  if (cache.threadHarvesting) return cache.threadHarvesting;
  cache.threadHarvesting = (async () => {
    const session = await ensureRoute(routeFor(request), cache);
    let idleRounds = 0;
    for (let round = 0; round < 80; round += 1) {
      if (!cache.session || session.generation !== cache.session.generation) return;
      await sendContent(session.tabId, { type: "expand_text" }, 5);
      await sendContent(session.tabId, { type: "expand_thread" }, 5);
      const added = absorb(cache, await sendContent(session.tabId, { type: "posts" }, 8));
      idleRounds = added ? 0 : idleRounds + 1;
      const expected = Math.min(Math.max(Number(request.reply_count || 0) + 1, 8), 100);
      if (cache.posts.size >= expected || (cache.posts.size > 1 && idleRounds >= 14)) return;
      await sendContent(session.tabId, { type: "scroll", aggressive: false }, 5);
      await delay(added ? 90 : Math.min(180 + idleRounds * 100, 1000));
    }
  })().finally(() => {
    cache.threadHarvesting = null;
  });
  return cache.threadHarvesting;
}

async function collectThread(request, routeKey) {
  const cache = cacheFor(routeKey);
  const harvest = startThreadHarvest(request, cache);
  const deadline = Date.now() + (cache.posts.size ? 100 : 1600);
  while (cache.posts.size < 2 && Date.now() < deadline && cache.threadHarvesting) {
    await delay(50);
  }
  void harvest;
  const posts = [...cache.posts.values()];
  const root = posts.findIndex((post) => post.id === request.conversation_id);
  if (root > 0) posts.unshift(posts.splice(root, 1)[0]);
  return posts;
}

async function closeTransports(immediate) {
  clearTimeout(detachTimer);
  if (!immediate) {
    detachTimer = setTimeout(() => void closeTransports(true), 30000);
    return;
  }
  await Promise.all([...routeCaches.values()].map((cache) => closeSession(cache.session)));
}

async function handleNative(request) {
  try {
    if (request.op === "status") {
      respond(request.id, {
        connected: true,
        tab_open: await tabExists(),
        extension_id: chrome.runtime.id,
        extension_version: chrome.runtime.getManifest().version,
      });
      return;
    }
    if (request.op === "shutdown") {
      await closeTransports(true);
      respond(request.id, { closed: true });
      return;
    }
    if (request.op === "detach") {
      await closeTransports(false);
      respond(request.id, { scheduled: true });
      return;
    }
    const routeKey = routeKeyFor(request);
    if (request.op === "me") {
      const cache = cacheFor(routeKey);
      const session = await ensureRoute(routeFor(request), cache);
      const me = await sendContent(session.tabId, { type: "me" });
      if (!me.username) throw new Error("X is not signed in in this browser profile");
      respond(request.id, me);
    } else if (request.op === "lists") {
      const cache = cacheFor(routeKey);
      const session = await ensureRoute(routeFor(request), cache);
      respond(request.id, await sendContent(session.tabId, { type: "lists" }));
    } else if (request.op === "thread") {
      respond(request.id, await collectThread(request, routeKey));
    } else if (request.op === "user") {
      const page = await collectPage({ ...request, op: "user_posts", cursor: null }, routeKey);
      const requested = String(request.user_id || "").replace(/^@/, "").toLowerCase();
      const post = page.items.find((item) => item.username.toLowerCase() === requested) || page.items[0];
      respond(request.id, post ? {
        id: post.username,
        name: post.name,
        username: post.username,
        verified: post.verified,
        description: "",
        profile_image_url: null,
        public_metrics: null,
      } : { id: requested, name: requested, username: requested, verified: false, description: "", profile_image_url: null, public_metrics: null });
    } else {
      respond(request.id, await collectPage(request, routeKey));
    }
  } catch (error) {
    respond(request.id, null, String(error?.message || error));
  }
}
