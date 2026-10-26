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

/// Run by the host through ExecuteScript (the "probe" message in the test page).
/// The completion value of the last expression becomes the JSON result.
pub const PROBE_JS: &str = r#"({
  title: document.title,
  url: location.href,
  viewport: [innerWidth, innerHeight],
  devicePixelRatio,
})"#;

/// Dark, matching BACKGROUND_RGB in gui.rs, so loading it causes no color switch.
/// Test page for: NewWindowRequested, WindowCloseRequested, the WebMessage bridge,
/// and navigation filtering.
pub const TEST_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>vwnd test</title>
<style>
  :root { color-scheme: dark; }
  body { font-family: system-ui, sans-serif; margin: 24px; background: #202020; color: #e6e6e6; }
  a { color: #8ab4f8; }
  h3 { margin: 20px 0 8px; }
  button { margin: 2px 4px 2px 0; }
  #log { background: #2a2a2a; padding: 8px; min-height: 4em; white-space: pre-wrap; }
</style></head>
<body>
  <h3>New window</h3>
  <a href="https://example.com/" target="_blank">target=_blank link</a><br>
  <button onclick="window.open('https://example.org/')">window.open</button>
  <button onclick="window.open('https://example.net/', 'popup', 'width=400,height=300')">window.open 400x300</button>
  <button onclick="window.open('https://example.net/?pos', 'popup2', 'left=80,top=80,width=400,height=300')">window.open at (80, 80)</button>
  <button onclick="setTimeout(() => window.open('https://example.com/?delayed'), 1500)">delayed (no gesture)</button>
  <button onclick="openSelfTest()">popup with this page</button>

  <h3>Window close</h3>
  <button onclick="window.close()">window.close()</button>
  <small>(works in popups opened by script)</small>

  <h3>Bridge</h3>
  <button onclick="vwnd.post({ type: 'ping', at: Date.now() })">post JSON</button>
  <button onclick="vwnd.post('hello from page')">post string</button>
  <button onclick="vwnd.post('probe')">ask host to probe this page</button>
  <div id="log"></div>

  <h3>Permissions</h3>
  <button onclick="ask('geolocation', () => new Promise((ok, err) =>
    navigator.geolocation.getCurrentPosition(ok, err)))">geolocation</button>
  <button onclick="ask('camera', () => navigator.mediaDevices.getUserMedia({ video: true }))">camera</button>
  <button onclick="ask('clipboard read', () => navigator.clipboard.readText())">clipboard read</button>

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
  // Runs a permission-gated API and logs allowed / the error (missing APIs included)
  function ask(name, request) {
    Promise.resolve()
      .then(request)
      .then(() => log(name + ': allowed'), (e) => log(name + ': ' + (e.name || e.message || e)));
  }
  // Popup that shows this page, so window.close() can be tested in a script-opened window
  function openSelfTest() {
    const w = window.open('', 'selftest', 'width=640,height=480');
    if (w) { w.document.write(document.documentElement.outerHTML); w.document.close(); }
  }
</script>
</body></html>"#;

/// Shown when the renderer fails again right after a reload (see RELOAD_COOLDOWN).
pub const CRASH_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Page crashed</title>
<style>
  :root { color-scheme: dark; }
  body { font-family: system-ui, sans-serif; margin: 48px; background: #202020; color: #e6e6e6; }
</style></head>
<body>
  <h2>The page stopped working</h2>
  <p>It crashed again right after reloading, so it was not reloaded automatically.</p>
</body></html>"#;

/// Shown when a page can't be reached (network or certificate errors).
/// `uri` comes from the network side, so it is HTML-escaped before it goes into the page.
pub fn error_page_html(uri: &str, status: &str) -> String {
    let uri_html = html_escape(uri);
    let status_html = html_escape(status);
    // a retry link only for http(s): anything else was never meant to be navigated to again
    let lower = uri.to_ascii_lowercase();
    let retry = if lower.starts_with("http://") || lower.starts_with("https://") {
        format!(r#"<p><a href="{uri_html}">Try again</a></p>"#)
    } else {
        String::new()
    };
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Can't reach this page</title>
<style>
  :root {{ color-scheme: dark; }}
  body {{ font-family: system-ui, sans-serif; margin: 48px; background: #202020; color: #e6e6e6; }}
  a {{ color: #8ab4f8; }}
  code {{ word-break: break-all; }}
</style></head>
<body>
  <h2>Can't reach this page</h2>
  <p><code>{uri_html}</code></p>
  <p>Error: <code>{status_html}</code></p>
  {retry}
</body></html>"#
    )
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_escapes_markup_and_quotes() {
        assert_eq!(
            html_escape(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
        assert_eq!(html_escape("plain"), "plain");
    }

    #[test]
    fn error_page_escapes_the_uri() {
        let html = error_page_html("https://x.y/<script>alert(1)</script>", "TIMEOUT");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn error_page_offers_retry_only_for_http() {
        assert!(error_page_html("https://x.y/", "TIMEOUT").contains("Try again"));
        assert!(error_page_html("HTTP://x.y/", "TIMEOUT").contains("Try again"));
        assert!(!error_page_html("about:blank", "TIMEOUT").contains("Try again"));
    }
}
