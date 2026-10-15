// ============================================================
// demo
// - bridge bootstrap script injected into every document
// - built-in test page for the features in gui.rs
// ============================================================

/// Injected before page scripts in every document (AddScriptToExecuteOnDocumentCreated).
/// Exposes `window.vwnd`:
/// - `vwnd.post(value)`: send any JSON-serializable value (or string) to the host
/// - `vwnd.on(fn)`: receive host messages, returns an unsubscribe function
pub const BRIDGE_JS: &str = r#"(() => {
  const wv = window.chrome && window.chrome.webview;
  if (!wv || window.vwnd) return;
  const listeners = new Set();
  wv.addEventListener('message', (e) => {
    for (const fn of listeners) {
      try { fn(e.data); } catch (err) { console.error('[vwnd]', err); }
    }
  });
  Object.defineProperty(window, 'vwnd', {
    value: Object.freeze({
      post: (value) => wv.postMessage(value),
      on: (fn) => { listeners.add(fn); return () => listeners.delete(fn); },
    }),
  });
})();"#;

/// Test page for: NewWindowRequested, WindowCloseRequested, the WebMessage bridge,
/// and navigation filtering.
pub const TEST_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>vwnd test</title>
<style>
  body { font-family: system-ui, sans-serif; margin: 24px; }
  h3 { margin: 20px 0 8px; }
  button { margin: 2px 4px 2px 0; }
  #log { background: #f4f4f4; padding: 8px; min-height: 4em; white-space: pre-wrap; }
</style></head>
<body>
  <h3>New window</h3>
  <a href="https://example.com/" target="_blank">target=_blank link</a><br>
  <button onclick="window.open('https://example.org/')">window.open</button>
  <button onclick="window.open('https://example.net/', 'popup', 'width=400,height=300')">window.open 400x300</button>
  <button onclick="setTimeout(() => window.open('https://example.com/?delayed'), 1500)">delayed (no gesture)</button>
  <button onclick="openSelfTest()">popup with this page</button>

  <h3>Window close</h3>
  <button onclick="window.close()">window.close()</button>
  <small>(works in popups opened by script)</small>

  <h3>Bridge</h3>
  <button onclick="vwnd.post({ type: 'ping', at: Date.now() })">post JSON</button>
  <button onclick="vwnd.post('hello from page')">post string</button>
  <div id="log"></div>

  <h3>Navigation</h3>
  <a href="https://www.google.com/">google.com</a> ·
  <a href="https://example.com/">example.com</a> ·
  <a href="mailto:someone@example.com">mailto:</a>

<script>
  const log = (s) => { document.getElementById('log').textContent += s + '\n'; };
  if (window.vwnd) {
    vwnd.on((data) => log('host -> page: ' + JSON.stringify(data)));
    log('bridge ready');
  } else {
    log('bridge not injected');
  }
  // Popup that shows this page, so window.close() can be tested in a script-opened window
  function openSelfTest() {
    const w = window.open('', 'selftest', 'width=640,height=480');
    if (w) { w.document.write(document.documentElement.outerHTML); w.document.close(); }
  }
</script>
</body></html>"#;