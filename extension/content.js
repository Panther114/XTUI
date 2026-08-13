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
observer.observe(document.documentElement, { childList: true, subtree: true });
scanPosts();

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
});
