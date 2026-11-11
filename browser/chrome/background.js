// Shed's browser extension. Shed asks (through its native host, `shed
// browser-host`) to open a link; this finds a tab already showing it and
// brings it forward, or opens a new one.

const HOST = "dev.shed.browser";
let port = null;

function connect() {
  if (port) return;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch (e) {
    port = null;
    return;
  }
  port.onMessage.addListener(onRequest);
  port.onDisconnect.addListener(() => {
    port = null;
    // Shed's host is not installed or stopped: try again later.
    setTimeout(connect, 30000);
  });
}

// The part of a URL that says which page it is: without query or fragment.
function base(url) {
  try {
    const u = new URL(url);
    return u.origin + u.pathname;
  } catch (e) {
    return url;
  }
}

async function onRequest(msg) {
  const reply = { id: msg.id, reused: false };
  try {
    if (!msg.new) {
      const want = base(msg.open);
      const tabs = await chrome.tabs.query({});
      // The tab whose page is the link, or under it; the most recently used first.
      const matches = tabs
        .filter((t) => t.url && base(t.url).startsWith(want))
        .sort((a, b) => (b.lastAccessed || 0) - (a.lastAccessed || 0));
      if (matches.length > 0) {
        const tab = matches[0];
        await chrome.tabs.update(tab.id, { active: true });
        await chrome.windows.update(tab.windowId, { focused: true });
        reply.reused = true;
      }
    }
    if (!reply.reused) {
      const tab = await chrome.tabs.create({ url: msg.open, active: true });
      await chrome.windows.update(tab.windowId, { focused: true });
    }
    reply.ok = true;
  } catch (e) {
    reply.ok = false;
    reply.error = String(e);
  }
  if (port) port.postMessage(reply);
}

chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
connect();
