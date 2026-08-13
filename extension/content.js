"use strict";

const metricNumber = (text) => {
  const match = (text || "").replaceAll(",", "").match(/([0-9.]+)\s*([KMB])?/i);
  if (!match) return 0;
  const multiplier = { K: 1e3, M: 1e6, B: 1e9 }[match[2]?.toUpperCase()] || 1;
  return Math.round(Number(match[1]) * multiplier);
};

let cachedLocation = location.href;
const postCache = new Map();
const MAX_POST_CACHE = 800;
const XTUI_CHANNEL = "xtui.timeline.v3";
let currentRouteKey = null;
let currentOperation = null;
let replaySequence = 0;
const replayWaiters = new Map();
const queuedCaptures = [];

function findCursor(root, wanted) {
  let found = null;
  const visit = (value) => {
    if (found || !value || typeof value !== "object") return;
    if (value.cursorType === wanted && typeof value.value === "string") {
      found = value.value;
      return;
    }
    for (const child of Array.isArray(value) ? value : Object.values(value)) visit(child);
  };
  visit(root);
  return found;
}

function unwrapResult(value) {
  let result = value;
  for (let depth = 0; depth < 6 && result && typeof result === "object"; depth += 1) {
    if (result.result && typeof result.result === "object") result = result.result;
    else if (result.tweet && typeof result.tweet === "object") result = result.tweet;
    else break;
  }
  return result;
}

function jsonMedia(legacy) {
  return (legacy?.extended_entities?.media || []).map((item) => {
    const video = item.type === "video" || item.type === "animated_gif";
    const variants = item.video_info?.variants || [];
    const best = variants
      .filter((variant) => variant.content_type === "video/mp4" && variant.url)
      .sort((left, right) => Number(right.bitrate || 0) - Number(left.bitrate || 0))[0];
    return {
      kind: video ? "video" : "photo",
      url: video ? best?.url || item.media_url_https : item.media_url_https,
      alt: item.ext_alt_text || null,
    };
  }).filter((item) => item.url);
}

function jsonPost(value, quotedIds) {
  const tweet = unwrapResult(value);
  const legacy = tweet?.legacy;
  const id = String(tweet?.rest_id || legacy?.id_str || "");
  if (!id || !legacy || (!legacy.full_text && !legacy.text)) return null;
  const user = unwrapResult(tweet.core?.user_results || tweet.author_results || legacy.user_results);
  const userLegacy = user?.legacy || {};
  const username = userLegacy.screen_name || user?.core?.screen_name || "";
  if (!username) return null;
  const quotedValue = tweet.quoted_status_result || legacy.quoted_status_result;
  const quoted = quotedValue ? jsonPost(quotedValue, quotedIds) : null;
  if (quoted) quotedIds.add(quoted.id);
  const noteText = tweet.note_tweet?.note_tweet_results?.result?.text;
  let createdAt = null;
  if (legacy.created_at) {
    const parsed = new Date(legacy.created_at);
    if (!Number.isNaN(parsed.valueOf())) createdAt = parsed.toISOString();
  }
  return {
    id,
    text: noteText || legacy.full_text || legacy.text || "",
    name: userLegacy.name || user?.core?.name || username,
    username,
    verified: Boolean(user?.is_blue_verified || userLegacy.verified),
    created_at: createdAt,
    replies: Number(legacy.reply_count || 0),
    reposts: Number(legacy.retweet_count || 0),
    likes: Number(legacy.favorite_count || 0),
    views: Number(tweet.views?.count || 0),
    media: jsonMedia(legacy),
    quoted,
  };
}

function normalizeTimeline(payload) {
  const posts = new Map();
  const quotedIds = new Set();
  const visit = (value) => {
    if (!value || typeof value !== "object") return;
    const post = jsonPost(value, quotedIds);
    if (post && !posts.has(post.id)) posts.set(post.id, post);
    for (const child of Array.isArray(value) ? value : Object.values(value)) visit(child);
  };
  visit(payload);
  for (const id of quotedIds) posts.delete(id);
  return {
    posts: [...posts.values()],
    top_cursor: findCursor(payload, "Top"),
    bottom_cursor: findCursor(payload, "Bottom"),
  };
}

function operationMatchesRoute(operation, route) {
  const name = String(operation || "").toLowerCase();
  const expected = String(route || "").toLowerCase();
  if (!name || !expected) return false;
  if (expected === "home") return name.includes("home") && name.includes("timeline");
  if (expected === "search") return name.includes("searchtimeline");
  if (expected === "bookmarks") return name.includes("bookmark");
  if (expected === "mentions") return name.includes("notification") || name.includes("mention");
  if (expected === "user" || expected === "user_posts") return name.includes("usertweets");
  if (expected === "likes") return name.includes("likes");
  if (expected === "list_posts") return name.includes("list") && name.includes("timeline");
  if (expected === "thread") return name.includes("tweetdetail");
  return false;
}

function deliverCapture(batch) {
  if (!currentRouteKey || !currentOperation) {
    queuedCaptures.push(batch);
    if (queuedCaptures.length > 8) queuedCaptures.shift();
    return;
  }
  // Operation names are private implementation details and change often.
  // Prefer the known route name, but accept a cursor-bearing timeline batch
  // from the configured route so cosmetic X renames do not disable replay.
  if (!operationMatchesRoute(batch.operation, currentOperation) && !batch.bottom_cursor) return;
  window.postMessage(
    { channel: XTUI_CHANNEL, kind: "accept", captureId: batch.capture_id },
    location.origin,
  );
  void chrome.runtime.sendMessage({
    type: "xtui-timeline-capture",
    route_key: currentRouteKey,
    batch,
  }).catch(() => {});
}

window.addEventListener("message", (event) => {
  if (event.source !== window || event.origin !== location.origin) return;
  const message = event.data;
  if (message?.channel !== XTUI_CHANNEL) return;
  if (message.kind === "capture") {
    const normalized = normalizeTimeline(message.payload);
    if (!normalized.posts.length) return;
    deliverCapture({
      ...normalized,
      operation: message.operation,
      capture_id: message.captureId,
      direction: message.requestCursor ? "tail" : "head",
      replay_id: message.replayId || null,
    });
  } else if (message.kind === "replay-result") {
    const waiter = replayWaiters.get(message.replayId);
    if (!waiter) return;
    replayWaiters.delete(message.replayId);
    waiter(message.ok ? { ok: true, value: true } : { ok: false, error: message.error });
  }
});

function replayTimeline(cursor) {
  const replayId = `replay-${Date.now()}-${++replaySequence}`;
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      replayWaiters.delete(replayId);
      resolve({ ok: false, error: "timeline replay timed out" });
    }, 10000);
    replayWaiters.set(replayId, (result) => {
      clearTimeout(timer);
      resolve(result);
    });
    window.postMessage(
      { channel: XTUI_CHANNEL, kind: "replay", replayId, cursor: cursor || null },
      location.origin,
    );
  });
}

function extractPost(article) {
  const time = article.querySelector("time");
  const statusLink =
    time?.closest("a")?.getAttribute("href") ||
    [...article.querySelectorAll('a[href*="/status/"]')]
      .map((element) => element.getAttribute("href"))
      .find((href) => /\/status\/\d+/.test(href || "")) ||
    "";
  const match = statusLink.match(/\/([^/]+)\/status\/(\d+)/);
  const user = article.querySelector('[data-testid="User-Name"]');
  const userSpans = [...(user?.querySelectorAll("span") || [])].map((span) =>
    span.textContent.trim()
  );
  const username =
    userSpans.find((text) => text.startsWith("@"))?.slice(1) || match?.[1] || "";
  const name =
    userSpans.find((text) => text && !text.startsWith("@") && text !== "Â·") || username;
  const metric = (testId) =>
    metricNumber(article.querySelector(`[data-testid="${testId}"]`)?.getAttribute("aria-label"));
  const media = [...article.querySelectorAll('[data-testid="tweetPhoto"] img')].map(
    (image) => ({
      kind: "photo",
      url: image.currentSrc || image.src,
      alt: image.alt || null,
    })
  );
  const video = article.querySelector("video");
  if (video?.poster) media.push({ kind: "video", url: video.poster, alt: "Video preview" });
  const quote = article.querySelector('[data-testid="quoteTweet"]');
  return {
    id: match?.[2] || "",
    text: article.querySelector('[data-testid="tweetText"]')?.innerText || "",
    name,
    username,
    verified: Boolean(
      user?.querySelector(
        '[data-testid="icon-verified"],img[alt="Verified account"],svg[aria-label="Verified account"]'
      )
    ),
    created_at: time?.dateTime || null,
    replies: metric("reply"),
    reposts: metric("retweet"),
    likes: metric("like"),
    views: metricNumber(
      article.querySelector('a[href$="/analytics"]')?.getAttribute("aria-label")
    ),
    media,
    quoted: quote ? extractPost(quote) : null,
  };
}

function resetForNavigation() {
  if (cachedLocation === location.href) return;
  cachedLocation = location.href;
  postCache.clear();
}

function scanPosts(root = document) {
  resetForNavigation();
  const articles = [];
  if (root.matches?.('article[data-testid="tweet"]')) articles.push(root);
  articles.push(...(root.querySelectorAll?.('article[data-testid="tweet"]') || []));
  for (const article of articles) {
    const post = extractPost(article);
    if (post.id && post.username) {
      postCache.set(post.id, post);
      while (postCache.size > MAX_POST_CACHE) {
        postCache.delete(postCache.keys().next().value);
      }
    }
  }
}

function collectPosts() {
  scanPosts();
  return [...postCache.values()];
}

function expandLongPosts(root = document) {
  let clicked = 0;
  for (const button of root.querySelectorAll('[data-testid="tweet-text-show-more-link"]')) {
    if (button.getAttribute("aria-disabled") === "true") continue;
    button.click();
    clicked += 1;
  }
  return clicked;
}

const observer = new MutationObserver((mutations) => {
  for (const mutation of mutations) {
    for (const node of mutation.addedNodes) {
      if (node.nodeType === Node.ELEMENT_NODE) scanPosts(node);
    }
  }
});
function startDomFallback() {
  if (!document.documentElement) return;
  observer.observe(document.documentElement, { childList: true, subtree: true });
  scanPosts();
}
if (document.documentElement) startDomFallback();
else document.addEventListener("DOMContentLoaded", startDomFallback, { once: true });

function collectSelf() {
  const profile =
    document.querySelector('a[data-testid="AppTabBar_Profile_Link"]') ||
    document.querySelector('nav[aria-label="Primary"] a[aria-label="Profile"]') ||
    [...document.querySelectorAll("header nav a,header a")].find((anchor) => {
      const href = anchor.getAttribute("href") || "";
      const slug = href.slice(1);
      return (
        /^\/[A-Za-z0-9_]+$/.test(href) &&
        !["home", "explore", "notifications", "messages", "i"].includes(slug)
      );
    });
  const username = (profile?.getAttribute("href") || "").split("/").filter(Boolean)[0] || "";
  const name =
    document
      .querySelector('[data-testid="SideNav_AccountSwitcher_Button"] img')
      ?.getAttribute("alt") ||
    document.querySelector('button[aria-label="Account menu"] img')?.getAttribute("alt") ||
    username;
  return { id: username, name, username, verified: false, description: "", profile_image_url: null, public_metrics: null };
}

function collectLists() {
  const seen = new Set();
  return [...document.querySelectorAll('a[href*="/lists/"]')]
    .map((anchor) => {
      const match = (anchor.getAttribute("href") || "").match(/\/lists\/(\d+)/);
      if (!match || seen.has(match[1])) return null;
      seen.add(match[1]);
      const text = anchor.innerText.trim().split("\n");
      return {
        id: match[1],
        name: text[0] || "List",
        description: text.slice(1).join(" "),
        member_count: null,
        follower_count: null,
        private: false,
      };
    })
    .filter(Boolean);
}

function expandThread() {
  const replyPattern = /show.*repl|view.*repl|more repl|additional repl|probable spam|offensive content|显示.*回复|更多回复|查看更多/i;
  let clicked = 0;
  for (const element of document.querySelectorAll('button,[role="button"]')) {
    const label = `${element.getAttribute("aria-label") || ""} ${element.textContent || ""}`.trim();
    if (replyPattern.test(label)) {
      element.click();
      clicked += 1;
    }
  }
  return clicked;
}

function quiescePage() {
  if (!document.getElementById("xtui-quiet-style")) {
    const style = document.createElement("style");
    style.id = "xtui-quiet-style";
    style.textContent = "*,*::before,*::after{animation:none!important;transition:none!important} video{visibility:hidden!important}";
    document.documentElement.appendChild(style);
  }
  for (const video of document.querySelectorAll("video")) {
    video.pause();
    video.preload = "none";
    video.muted = true;
  }
}

chrome.runtime.onMessage.addListener((message, _sender, respond) => {
  if (message.type === "configure") {
    currentRouteKey = message.route_key || null;
    currentOperation = message.operation || null;
    while (currentRouteKey && queuedCaptures.length) deliverCapture(queuedCaptures.shift());
    respond({ ok: true, value: true });
    return false;
  }
  if (message.type === "replay") {
    void replayTimeline(message.cursor || null).then(respond);
    return true;
  }
  try {
    quiescePage();
    if (message.type === "posts") respond({ ok: true, value: collectPosts() });
    else if (message.type === "expand_text") {
      respond({ ok: true, value: expandLongPosts() });
    }
    else if (message.type === "me") respond({ ok: true, value: collectSelf() });
    else if (message.type === "lists") respond({ ok: true, value: collectLists() });
    else if (message.type === "expand_thread") {
      respond({ ok: true, value: expandThread() });
    } else if (message.type === "scroll") {
      const multiplier = message.aggressive ? 2.8 : 1.45;
      window.scrollBy({ top: Math.max(window.innerHeight * multiplier, 1100), behavior: "auto" });
      respond({ ok: true, value: true });
    } else if (message.type === "feed") {
      const label = message.feed === "for_you" ? "For You" : "Following";
      const tab = [...document.querySelectorAll('[role="tab"]')].find(
        (element) => element.textContent.trim() === label
      );
      if (tab && tab.getAttribute("aria-selected") !== "true") tab.click();
      respond({ ok: true, value: Boolean(tab) });
    } else respond({ ok: false, error: "unknown content request" });
  } catch (error) {
    respond({ ok: false, error: String(error?.message || error) });
  }
  return false;
});
