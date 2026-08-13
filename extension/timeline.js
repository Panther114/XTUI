"use strict";

(() => {
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

  function mediaFromLegacy(legacy) {
    return (legacy?.extended_entities?.media || []).map((item) => {
      const video = item.type === "video" || item.type === "animated_gif";
      const best = (item.video_info?.variants || [])
        .filter((variant) => variant.content_type === "video/mp4" && variant.url)
        .sort((left, right) => Number(right.bitrate || 0) - Number(left.bitrate || 0))[0];
      return {
        kind: video ? "video" : "photo",
        url: video ? best?.url || item.media_url_https : item.media_url_https,
        alt: item.ext_alt_text || null,
      };
    }).filter((item) => item.url);
  }

  function postFromJson(value, quotedIds) {
    const tweet = unwrapResult(value);
    const legacy = tweet?.legacy;
    const id = String(tweet?.rest_id || legacy?.id_str || "");
    if (!id || !legacy || (!legacy.full_text && !legacy.text)) return null;
    const user = unwrapResult(tweet.core?.user_results || tweet.author_results || legacy.user_results);
    const userLegacy = user?.legacy || {};
    const username = userLegacy.screen_name || user?.core?.screen_name || "";
    if (!username) return null;
    const quotedValue = tweet.quoted_status_result || legacy.quoted_status_result;
    const quoted = quotedValue ? postFromJson(quotedValue, quotedIds) : null;
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
      media: mediaFromLegacy(legacy),
      quoted,
    };
  }

  function normalizeTimeline(payload) {
    const posts = new Map();
    const quotedIds = new Set();
    const visit = (value) => {
      if (!value || typeof value !== "object") return;
      const post = postFromJson(value, quotedIds);
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

  globalThis.XtuiTimeline = Object.freeze({ normalizeTimeline });
})();
