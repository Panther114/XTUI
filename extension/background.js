"use strict";

importScripts("timeline.js");

const HOST = "com.xtui.bridge";
const PAGE_SIZE = 12;
const HARVEST_BATCH = 48;
const PREFETCH_RESERVOIR = 144;
const MAX_CACHED_POSTS = 800;
// DOM scrolling is only a passive last resort. Long scroll loops are both
// throttled in hidden tabs and startling if the user later opens that tab.
const MAX_SCROLL_ROUNDS = 4;
const FIRST_POST_TIMEOUT_MS = 3500;
const NEXT_PAGE_WAIT_MS = 1400;

let nativePort = null;
let detachTimer = null;
const routeCaches = new Map();
const X_WEB_BEARER = "Bearer AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
let directClient = null;
let directClientLoading = null;
const persistedCacheKey = (routeKey) => `route:${routeKey}`;

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

function initialStateFromHtml(html) {
  const marker = "__INITIAL_STATE__=";
  const start = html.indexOf(marker);
  if (start < 0) throw new Error("X bootstrap state was not present");
  const valueStart = start + marker.length;
  const valueEnd = html.indexOf(";window.__META_DATA__", valueStart);
  if (valueEnd < 0) throw new Error("X bootstrap state was incomplete");
  return JSON.parse(html.slice(valueStart, valueEnd));
}

function quotedNames(source) {
  return [...source.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

async function xHeaders() {
  const csrf = await chrome.cookies.get({ url: "https://x.com/", name: "ct0" });
  if (!csrf?.value) throw new Error("X is not signed in in this browser profile");
  return {
    authorization: X_WEB_BEARER,
    "content-type": "application/json",
    "x-csrf-token": csrf.value,
    "x-twitter-active-user": "yes",
    "x-twitter-auth-type": "OAuth2Session",
    "x-twitter-client-language": "en",
  };
}

async function xFetch(url) {
  const response = await fetch(url, { credentials: "include", headers: await xHeaders() });
  if (!response.ok) throw new Error(`X web request failed (${response.status})`);
  return response;
}

function operationFromChunk(chunk, operationName) {
  const marker = `operationName:"${operationName}"`;
  const at = chunk.indexOf(marker);
  if (at < 0) throw new Error(`X client omitted ${operationName}`);
  const start = chunk.lastIndexOf("queryId:", at);
  const end = chunk.indexOf("}}}", at) + 3;
  const definition = chunk.slice(start, end);
  const queryId = definition.match(/queryId:"([^"]+)"/)?.[1];
  const featureSource = definition.match(/featureSwitches:\[([^\]]*)\]/)?.[1] || "";
  const fieldSource = definition.match(/fieldToggles:\[([^\]]*)\]/)?.[1] || "";
  if (!queryId) throw new Error(`X client query id missing for ${operationName}`);
  return {
    queryId,
    operationName,
    featureNames: quotedNames(featureSource),
    fieldNames: quotedNames(fieldSource),
  };
}

function operationParameters(client, operation) {
  const fieldDefaults = {
    withAuxiliaryUserLabels: true,
    withArticleRichContentState: true,
  };
  return {
    features: Object.fromEntries(
      operation.featureNames.map((name) => [name, Boolean(client.switches[name]?.value)]),
    ),
    fieldToggles: Object.fromEntries(
      operation.fieldNames.map((name) => [name, Boolean(fieldDefaults[name])]),
    ),
  };
}

async function loadDirectClient() {
  if (directClient) return directClient;
  if (directClientLoading) return directClientLoading;
  directClientLoading = (async () => {
    const homeResponse = await fetch("https://x.com/home", { credentials: "include" });
    if (!homeResponse.ok) throw new Error(`X bootstrap failed (${homeResponse.status})`);
    const html = await homeResponse.text();
    const state = initialStateFromHtml(html);
    if (!state.session?.user_id) throw new Error("X is not signed in in this browser profile");

    const chunkName = "shared~bundle.LoggedInMain~bundle.HomeTimeline";
    const chunkId = html.match(new RegExp(`(\\d+):"${chunkName}"`))?.[1];
    if (!chunkId) throw new Error("X Home timeline bundle was not discoverable");
    const hashMatches = [...html.matchAll(new RegExp(`${chunkId}:"([a-f0-9]{7,})"`, "g"))];
    const hash = hashMatches.at(-1)?.[1];
    if (!hash) throw new Error("X Home timeline bundle hash was not discoverable");
    const chunkUrl = `https://abs.twimg.com/responsive-web/client-web/${chunkName}.${hash}a.js`;
    const chunkResponse = await fetch(chunkUrl);
    if (!chunkResponse.ok) throw new Error(`X Home timeline bundle failed (${chunkResponse.status})`);
    const homeChunk = await chunkResponse.text();
    const homeLatest = operationFromChunk(homeChunk, "HomeLatestTimeline");
    let homeRanked = null;
    try {
      homeRanked = operationFromChunk(homeChunk, "HomeTimeline");
    } catch (_error) {}
    const switches = state.featureSwitch || {};
    const mainUrl = html.match(/https:\/\/abs\.twimg\.com\/responsive-web\/client-web\/main\.[^"']+\.js/)?.[0];
    const user = state.entities?.users?.entities?.[state.session.user_id];
    directClient = {
      homeLatest,
      homeRanked,
      mainUrl,
      operations: new Map(),
      switches,
      me: {
        id: String(state.session.user_id),
        name: user?.name || user?.core?.name || user?.screen_name || "X user",
        username: user?.screen_name || user?.core?.screen_name || "",
        verified: Boolean(user?.verified || user?.is_blue_verified),
        description: user?.description || "",
        profile_image_url: user?.profile_image_url_https || null,
        public_metrics: null,
      },
    };
    return directClient;
  })().finally(() => {
    directClientLoading = null;
  });
  return directClientLoading;
}

async function loadMainOperation(operationName) {
  const client = await loadDirectClient();
  if (client.operations.has(operationName)) return client.operations.get(operationName);
  if (!client.mainUrl) throw new Error("X main client bundle was not discoverable");
  if (!client.mainChunk) {
    const response = await fetch(client.mainUrl);
    if (!response.ok) throw new Error(`X main client bundle failed (${response.status})`);
    client.mainChunk = await response.text();
  }
  const operation = operationFromChunk(client.mainChunk, operationName);
  client.operations.set(operationName, operation);
  return operation;
}

async function directOperation(operation, variables) {
  const client = await loadDirectClient();
  const parameters = operationParameters(client, operation);
  const query = new URLSearchParams({
    variables: JSON.stringify(variables),
    features: JSON.stringify(parameters.features),
    fieldToggles: JSON.stringify(parameters.fieldToggles),
  });
  const url = `https://x.com/i/api/graphql/${operation.queryId}/${operation.operationName}?${query}`;
  return (await xFetch(url)).json();
}

async function directHomeBatch(cursor = null, feed = "following", retry = true) {
  const client = await loadDirectClient();
  const ranked = feed === "for_you" && client.homeRanked;
  const operation = ranked || client.homeLatest;
  const variables = {
    count: 20,
    cursor: cursor || undefined,
    enableRanking: Boolean(ranked),
    includePromotedContent: true,
    requestContext: "launch",
    withCommunity: true,
  };
  try {
    return XtuiTimeline.normalizeTimeline(await directOperation(operation, variables));
  } catch (error) {
    if (!retry) throw error;
    directClient = null;
    return directHomeBatch(cursor, feed, false);
  }
}

async function restoreCache(cache, routeKey) {
  if (cache.restored) return cache.restored;
  cache.restored = (async () => {
    const storage = chrome.storage?.session;
    if (!storage) return;
    const key = persistedCacheKey(routeKey);
    const saved = (await storage.get(key))?.[key];
    if (!saved) return;
    absorb(cache, saved.posts, "tail");
    cache.topCursor = saved.topCursor || cache.topCursor;
    cache.bottomCursor = saved.bottomCursor || cache.bottomCursor;
    cache.revision = Math.max(cache.revision, Number(saved.revision || 0));
  })();
  return cache.restored;
}

function persistCache(cache, routeKey) {
  const storage = chrome.storage?.session;
  if (!storage) return Promise.resolve();
  const key = persistedCacheKey(routeKey);
  return storage.set({
    [key]: {
      posts: [...cache.posts.values()],
      topCursor: cache.topCursor,
      bottomCursor: cache.bottomCursor,
      revision: cache.revision,
    },
  });
}

function startDirectHomeFill(cache, routeKey, feed) {
  if (cache.jsonPrefilling) return cache.jsonPrefilling;
  cache.jsonPrefilling = (async () => {
    let cursor = cache.bottomCursor;
    let misses = 0;
    while (cache.posts.size < PREFETCH_RESERVOIR && cursor && misses < 2) {
      const batch = await directHomeBatch(cursor, feed);
      const added = absorb(cache, batch.posts, "tail");
      cursor = batch.bottom_cursor;
      cache.bottomCursor = cursor || cache.bottomCursor;
      cache.topCursor = batch.top_cursor || cache.topCursor;
      misses = added ? 0 : misses + 1;
    }
    await persistCache(cache, routeKey);
  })().finally(() => {
    cache.jsonPrefilling = null;
  });
  return cache.jsonPrefilling;
}

async function collectDirectHome(request, routeKey) {
  const cache = cacheFor(routeKey);
  await restoreCache(cache, routeKey);
  const priorSeen = parseCursor(request.cursor);
  if (!request.cursor || !pageFromCache(cache, priorSeen, 1).length) {
    const batch = await directHomeBatch(
      request.cursor ? cache.bottomCursor : null,
      request.feed || "following",
    );
    absorb(cache, batch.posts, request.cursor ? "tail" : "head");
    cache.topCursor = batch.top_cursor || cache.topCursor;
    cache.bottomCursor = batch.bottom_cursor || cache.bottomCursor;
  }
  await persistCache(cache, routeKey);
  void startDirectHomeFill(cache, routeKey, request.feed || "following");
  return makePage(cache, priorSeen);
}

async function collectDirectThread(request, routeKey) {
  const cache = cacheFor(routeKey);
  await restoreCache(cache, routeKey);
  if (!cache.posts.size) {
    const operation = await loadMainOperation("TweetDetail");
    const payload = await directOperation(operation, {
      focalTweetId: String(request.conversation_id),
      with_rux_injections: false,
      rankingMode: "Relevance",
      includePromotedContent: true,
      withCommunity: true,
      withQuickPromoteEligibilityTweetFields: true,
      withBirdwatchNotes: true,
      withVoice: true,
      withV2Timeline: true,
    });
    const batch = XtuiTimeline.normalizeTimeline(payload);
    absorb(cache, batch.posts, "tail");
    await persistCache(cache, routeKey);
  }
  const posts = [...cache.posts.values()];
  const root = posts.findIndex((post) => post.id === String(request.conversation_id));
  if (root > 0) posts.unshift(posts.splice(root, 1)[0]);
  return posts;
}

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
  if (message?.type === "xtui-timeline-capture") {
    const cache = routeCaches.get(message.route_key);
    if (cache) {
      const batch = message.batch || {};
      absorb(cache, batch.posts, batch.direction === "head" ? "head" : "tail");
      cache.topCursor = batch.top_cursor || cache.topCursor;
      cache.bottomCursor = batch.bottom_cursor || cache.bottomCursor;
      cache.replayReady = true;
      cache.lastJsonCapture = Date.now();
      if (batch.direction === "head") cache.lastHeadRefresh = Date.now();
      // Fill the in-memory reservoir immediately, before a captured request
      // can age and without waiting for the TUI to ask for the next page.
      void startJsonPrefill(cache);
    }
    reply({ accepted: Boolean(cache) });
    return false;
  }
  if (message?.type === "popup-status") {
    void tabExists().then((tab_open) =>
      reply({
        connected: Boolean(nativePort),
        tab_open,
        extension_version: chrome.runtime.getManifest().version,
      })
    );
    return true;
  }
  return false;
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
  // Bootstrap identity and the default Following feed share one /home tab.
  // This removes a duplicate x.com startup navigation and lets the initial
  // timeline response warm Home before the reader opens it.
  if (request.op === "me") {
    return JSON.stringify(["home", "", "", "", "", "following"]);
  }
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
      topCursor: null,
      bottomCursor: null,
      replayReady: false,
      lastJsonCapture: 0,
      lastHeadRefresh: 0,
      headRefreshing: null,
      jsonPrefilling: null,
      restored: null,
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

function absorb(cache, posts, direction = "tail") {
  let added = 0;
  const incoming = new Map();
  for (const post of posts || []) {
    if (!post?.id) continue;
    if (!cache.posts.has(post.id)) {
      cache.revision += 1;
      added += 1;
    }
    incoming.set(post.id, post);
  }
  cache.posts = direction === "head"
    ? new Map([
        ...incoming,
        ...[...cache.posts].filter(([id]) => !incoming.has(id)),
      ])
    : new Map([...cache.posts, ...incoming]);
  while (cache.posts.size > MAX_CACHED_POSTS) {
    const eviction = direction === "head"
      ? [...cache.posts.keys()].at(-1)
      : cache.posts.keys().next().value;
    cache.posts.delete(eviction);
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

async function ensureRoute(url, cache, routeKey) {
  clearTimeout(detachTimer);
  if (!cache.session) cache.session = { tabId: null, url, generation: 0 };
  const session = cache.session;
  const exists = await sessionExists(session);
  if (!exists) {
    // Put transport tabs in an existing normal window and explicitly preserve
    // its selected tab. This prevents opening or foregrounding an X window.
    const windows = await chrome.windows.getAll({ windowTypes: ["normal"] });
    const target = windows.find((window) => window.focused) || windows[0];
    const activeTabs = target
      ? await chrome.tabs.query({ active: true, windowId: target.id })
      : [];
    const tab = await chrome.tabs.create({
      active: false,
      url,
      ...(target ? { windowId: target.id } : {}),
    });
    session.tabId = tab.id;
    session.url = url;
    session.generation += 1;
    await chrome.tabs.update(session.tabId, { muted: true, autoDiscardable: false });
    if (activeTabs[0]?.id) {
      await chrome.tabs.update(activeTabs[0].id, { active: true }).catch(() => {});
    }
    // The document-start content bridge can identify the route before X has
    // finished loading, so its first timeline response is never lost.
    await sendContent(session.tabId, {
      type: "configure",
      route_key: routeKey,
      operation: JSON.parse(routeKey)[0],
    }, 30);
    await waitForComplete(session.tabId);
    return { ...session };
  }
  await sendContent(session.tabId, {
    type: "configure",
    route_key: routeKey,
    operation: JSON.parse(routeKey)[0],
  }, 30);
  return { ...session };
}

function startJsonPrefill(cache) {
  if (cache.jsonPrefilling) return cache.jsonPrefilling;
  cache.jsonPrefilling = (async () => {
    let misses = 0;
    while (
      cache.posts.size < PREFETCH_RESERVOIR &&
      cache.replayReady &&
      cache.bottomCursor &&
      await sessionExists(cache.session)
    ) {
      const revision = cache.revision;
      try {
        await sendContent(cache.session.tabId, { type: "replay", cursor: cache.bottomCursor }, 1);
        const added = await waitForRevision(cache, revision, 900);
        if (added > 0) {
          misses = 0;
          continue;
        }
      } catch (_error) {}
      misses += 1;
      if (misses >= 2) return;
    }
  })().finally(() => {
    cache.jsonPrefilling = null;
  });
  return cache.jsonPrefilling;
}

async function waitForRevision(cache, revision, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (cache.revision === revision && Date.now() < deadline) await delay(25);
  return cache.revision - revision;
}

function startHeadRefresh(request, cache, routeKey) {
  if (cache.headRefreshing) return cache.headRefreshing;
  if (!cache.replayReady || Date.now() - cache.lastHeadRefresh < 1500) {
    return Promise.resolve(0);
  }
  cache.headRefreshing = (async () => {
    const session = await ensureRoute(routeFor(request), cache, routeKey);
    const revision = cache.revision;
    await sendContent(session.tabId, { type: "replay", cursor: null }, 1);
    const added = await waitForRevision(cache, revision, 900);
    cache.lastHeadRefresh = Date.now();
    return added;
  })().catch(() => 0).finally(() => {
    cache.headRefreshing = null;
  });
  return cache.headRefreshing;
}

async function waitForPosts(cache, priorSeen, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (pageFromCache(cache, priorSeen, 1).length) return;
    if (!cache.harvesting) return;
    await delay(50);
  }
}

function startHarvest(request, cache, priorSeen, routeKey) {
  if (cache.harvesting) return cache.harvesting;
  cache.harvesting = (async () => {
    const url = routeFor(request);
    const session = await ensureRoute(url, cache, routeKey);
    const targetUnseen = Math.min(
      PREFETCH_RESERVOIR,
      Math.max(HARVEST_BATCH, MAX_CACHED_POSTS - priorSeen.length),
    );
    if (request.op === "home") {
      await sendContent(session.tabId, { type: "feed", feed: request.feed || "following" }, 12);
    }
    // A document-start capture starts this immediately, while its captured
    // request metadata is still fresh. It does not depend on layout, timers,
    // intersection observers, or the transport tab being focused.
    if (cache.jsonPrefilling) await cache.jsonPrefilling;
    if (cache.replayReady && cache.bottomCursor) await startJsonPrefill(cache);
    let idleRounds = 0;
    let replayMisses = 0;
    for (let round = 0; round < MAX_SCROLL_ROUNDS; round += 1) {
      if (!cache.session || session.generation !== cache.session.generation) return;
      if (cache.replayReady && cache.bottomCursor && replayMisses < 2) {
        const revision = cache.revision;
        try {
          await sendContent(session.tabId, { type: "replay", cursor: cache.bottomCursor }, 1);
          const replayed = await waitForRevision(cache, revision, 700);
          if (replayed > 0) {
            idleRounds = 0;
            replayMisses = 0;
            if (pageFromCache(cache, priorSeen, targetUnseen).length >= targetUnseen) return;
            continue;
          }
        } catch (_error) {}
        replayMisses += 1;
      }
      await sendContent(session.tabId, { type: "expand_text" }, 3);
      const added = absorb(cache, await sendContent(session.tabId, { type: "posts" }, 8), "tail");
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
    if (!request.cursor && request.op === "home") {
      await startHeadRefresh(request, cache, routeKey);
    }
    startHarvest(request, cache, priorSeen, routeKey);
    return makePage(cache, priorSeen);
  }

  const harvest = startHarvest(request, cache, priorSeen, routeKey);
  await waitForPosts(
    cache,
    priorSeen,
    request.cursor ? NEXT_PAGE_WAIT_MS : FIRST_POST_TIMEOUT_MS
  );
  // Do not await the full harvest. It deliberately continues behind the TUI.
  void harvest;
  return makePage(cache, priorSeen);
}

function startThreadHarvest(request, cache, routeKey) {
  if (cache.threadHarvesting) return cache.threadHarvesting;
  cache.threadHarvesting = (async () => {
    const session = await ensureRoute(routeFor(request), cache, routeKey);
    let idleRounds = 0;
    for (let round = 0; round < 80; round += 1) {
      if (!cache.session || session.generation !== cache.session.generation) return;
      await sendContent(session.tabId, { type: "expand_text" }, 5);
      await sendContent(session.tabId, { type: "expand_thread" }, 5);
      const added = absorb(cache, await sendContent(session.tabId, { type: "posts" }, 8), "tail");
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
  const harvest = startThreadHarvest(request, cache, routeKey);
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

async function releaseSecondaryTransports() {
  await Promise.all(
    [...routeCaches.entries()]
      .filter(([key]) => {
        try {
          return JSON.parse(key)[0] !== "home";
        } catch (_error) {
          return true;
        }
      })
      .map(([, cache]) => closeSession(cache.session)),
  );
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
      routeCaches.clear();
      if (chrome.storage?.session) await chrome.storage.session.clear();
      respond(request.id, { closed: true });
      return;
    }
    if (request.op === "release_secondary") {
      await releaseSecondaryTransports();
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
      const me = (await loadDirectClient()).me;
      if (!me.username) throw new Error("X is not signed in in this browser profile");
      respond(request.id, me);
    } else if (request.op === "home") {
      respond(request.id, await collectDirectHome(request, routeKey));
    } else if (request.op === "lists") {
      const cache = cacheFor(routeKey);
      const session = await ensureRoute(routeFor(request), cache, routeKey);
      respond(request.id, await sendContent(session.tabId, { type: "lists" }));
    } else if (request.op === "thread") {
      respond(request.id, await collectDirectThread(request, routeKey));
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
