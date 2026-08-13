"use strict";

// Runs in x.com's MAIN world before the application starts. The page remains
// responsible for authentication and request signing; XTUI only records a
// successful timeline request and replays that same shape with a new cursor.
const XTUI_CHANNEL = "xtui.timeline.v3";
const originalFetch = window.fetch.bind(window);
const candidates = new Map();
let acceptedTemplate = null;
let sequence = 0;

function isTimelineUrl(value) {
  try {
    const url = new URL(String(value), location.href);
    return url.origin === location.origin && url.pathname.includes("/i/api/graphql/");
  } catch (_error) {
    return false;
  }
}

function safeHeaders(headers) {
  const result = {};
  for (const [name, value] of new Headers(headers || {}).entries()) {
    if (!["content-length", "cookie", "host"].includes(name.toLowerCase())) result[name] = value;
  }
  return result;
}

function cursorFromJson(value) {
  if (!value || typeof value !== "object") return null;
  if (typeof value.cursor === "string") return value.cursor;
  if (value.variables && typeof value.variables.cursor === "string") return value.variables.cursor;
  return null;
}

function cursorFromTemplate(template) {
  try {
    const url = new URL(template.url, location.href);
    const variables = JSON.parse(url.searchParams.get("variables") || "{}");
    const cursor = cursorFromJson(variables);
    if (cursor) return cursor;
  } catch (_error) {}
  try {
    return cursorFromJson(JSON.parse(template.body || "{}"));
  } catch (_error) {
    return null;
  }
}

function withCursor(template, cursor) {
  const next = structuredClone(template);
  const apply = (variables) => {
    if (!variables || typeof variables !== "object") return variables;
    if (cursor) variables.cursor = cursor;
    else delete variables.cursor;
    return variables;
  };
  const url = new URL(next.url, location.href);
  if (url.searchParams.has("variables")) {
    const variables = apply(JSON.parse(url.searchParams.get("variables") || "{}"));
    url.searchParams.set("variables", JSON.stringify(variables));
    next.url = url.toString();
  } else if (next.body) {
    const body = JSON.parse(next.body);
    if (body.variables) apply(body.variables);
    else apply(body);
    next.body = JSON.stringify(body);
  }
  return next;
}

function post(kind, detail) {
  window.postMessage({ channel: XTUI_CHANNEL, kind, ...detail }, location.origin);
}

async function publish(template, response, replayId = null) {
  const contentType = response.headers.get("content-type") || "";
  if (!response.ok || !contentType.includes("json")) return false;
  let payload;
  try {
    payload = await response.clone().json();
  } catch (_error) {
    return false;
  }
  const captureId = `capture-${Date.now()}-${++sequence}`;
  candidates.set(captureId, template);
  while (candidates.size > 24) candidates.delete(candidates.keys().next().value);
  post("capture", {
    captureId,
    replayId,
    operation: new URL(template.url, location.href).pathname.split("/").at(-1) || "",
    requestCursor: cursorFromTemplate(template),
    payload,
  });
  return true;
}

window.fetch = async function xtuiFetch(input, init) {
  let template = null;
  let body = Promise.resolve(null);
  try {
    const request = new Request(input, init);
    if (isTimelineUrl(request.url)) {
      template = {
        url: request.url,
        method: request.method,
        headers: safeHeaders(request.headers),
        body: null,
      };
      if (!["GET", "HEAD"].includes(request.method)) body = request.clone().text();
    }
  } catch (_error) {}
  const response = await originalFetch(input, init);
  if (template) {
    void body.then((value) => {
      template.body = value;
      return publish(template, response);
    });
  }
  return response;
};

window.addEventListener("message", (event) => {
  if (event.source !== window || event.origin !== location.origin) return;
  const message = event.data;
  if (message?.channel !== XTUI_CHANNEL) return;
  if (message.kind === "accept") {
    const template = candidates.get(message.captureId);
    if (template) acceptedTemplate = template;
    return;
  }
  if (message.kind !== "replay") return;
  void (async () => {
    if (!acceptedTemplate) {
      post("replay-result", { replayId: message.replayId, ok: false, error: "no captured request template" });
      return;
    }
    try {
      const template = withCursor(acceptedTemplate, message.cursor || null);
      const response = await originalFetch(template.url, {
        method: template.method,
        headers: template.headers,
        body: template.body,
        credentials: "include",
      });
      const captured = await publish(template, response, message.replayId);
      if (!response.ok || !captured) throw new Error(`timeline replay returned HTTP ${response.status}`);
      post("replay-result", { replayId: message.replayId, ok: true });
    } catch (error) {
      post("replay-result", {
        replayId: message.replayId,
        ok: false,
        error: String(error?.message || error),
      });
    }
  })();
});
