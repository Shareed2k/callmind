use crate::styles::APP_CSS;

/// Render standard application HTML shell.
#[must_use]
pub fn render_layout(title: &str, active_nav: &str, body_html: &str) -> String {
    let calls_active = if active_nav == "calls" { "active" } else { "" };
    let analytics_active = if active_nav == "analytics" {
        "active"
    } else {
        ""
    };
    let ask_active = if active_nav == "ask" { "active" } else { "" };
    let escaped_title = html_escape::encode_text(title);

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{escaped_title} — CallMind</title>
  <style>{APP_CSS}</style>
</head>
<body>
  <div class="app-container">
    <header class="navbar">
      <div class="nav-brand">
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72 12.84 12.84 0 0 0 .7 2.81 2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45 12.84 12.84 0 0 0 2.81.7A2 2 0 0 1 22 16.92z"></path></svg>
        <span>CallMind</span>
      </div>
      <nav class="nav-links" style="align-items:center;">
        <a href="/calls" class="nav-link {calls_active}">Calls</a>
        <a href="/analytics" class="nav-link {analytics_active}">Analytics</a>
        <a href="/ask" class="nav-link {ask_active}">Ask AI</a>
        <a href="/swagger-ui" class="nav-link" target="_blank">API Docs</a>
        <div id="worker-status-pill" style="display:inline-flex; align-items:center; gap:0.4rem; background:rgba(0,0,0,0.3); border:1px solid var(--border-color); padding:0.25rem 0.65rem; border-radius:1rem; font-size:0.75rem; color:var(--text-secondary); font-family:monospace;">
          <span style="display:inline-block; width:8px; height:8px; border-radius:50%; background:#10b981;"></span>
          <span id="worker-status-text">Workers Active</span>
        </div>
      </nav>
    </header>
    <main class="main-content">
      {body_html}
    </main>
    <script>
      async function updateWorkerStatus() {{
        try {{
          const res = await fetch('/api/v1/system/metrics');
          if (res.ok) {{
            const data = await res.json();
            const textEl = document.getElementById('worker-status-text');
            if (textEl) {{
              if (data.running_jobs > 0 || data.pending_jobs > 0) {{
                textEl.innerHTML = `⚡ ${{data.active_workers}} Workers | 🔄 ${{data.running_jobs}} active | ⏳ ${{data.pending_jobs}} queue | ✓ ${{data.completed_calls}} done`;
              }} else {{
                textEl.innerHTML = `🟢 ${{data.active_workers}} Workers Idle | ✓ ${{data.completed_calls}} completed`;
              }}
            }}
          }}
        }} catch (e) {{}}
      }}
      updateWorkerStatus();
      setInterval(updateWorkerStatus, 2500);
    </script>
  </div>
</body>
</html>"#
    )
}
