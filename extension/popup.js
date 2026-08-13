"use strict";
const status = document.getElementById("status");
chrome.runtime.sendMessage({ type: "popup-status" }, (result) => {
  status.textContent = result?.connected
    ? result.tab_open
      ? "Connected · transport active"
      : "Connected · waiting for XTUI"
    : "Native host unavailable · run xtui extension install --edge";
});
