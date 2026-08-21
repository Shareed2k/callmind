use crate::views::layout::render_layout;
use callmind_search::AskCallsResponse;
use std::fmt::Write;

/// Render the Ask AI interactive page HTML.
#[must_use]
pub fn render_ask_page(query: Option<&str>, response: Option<&AskCallsResponse>) -> String {
    let query_val = html_escape::encode_text(query.unwrap_or(""));

    let response_html = response.map_or_else(
        String::new,
        |res| {
            let mut citations_html = String::new();
            for c in &res.citations {
                let snippet_escaped = html_escape::encode_text(&c.text_snippet);
                let _ = write!(
                    citations_html,
                    r#"<div style="background: rgba(0,0,0,0.2); padding: 0.75rem; border-radius: 0.25rem; margin-top: 0.5rem; border: 1px solid var(--border-color);">
                        <a href="/calls/{id}" style="color: #60a5fa; font-weight:600; font-family: monospace;">Call {id} &rarr;</a>
                        <p style="font-size: 0.85rem; color: var(--text-secondary); margin-top: 0.25rem;">"{snippet_escaped}"</p>
                      </div>"#,
                    id = c.call_id
                );
            }

            let answer_escaped = html_escape::encode_text(&res.answer);
            format!(
                r#"
                <div class="card" style="margin-top: 1.5rem; border-color: var(--accent-blue);">
                  <div class="card-title">AI Synthesized Answer</div>
                  <p style="font-size: 1.05rem; line-height: 1.6; margin-bottom: 1rem;">{answer_escaped}</p>
                  <h4 style="font-size: 0.85rem; color: var(--text-secondary); text-transform: uppercase;">Citations ({})</h4>
                  {}
                </div>
                "#,
                res.citations.len(),
                citations_html
            )
        },
    );

    let body = format!(
        r#"
        <div style="margin-bottom: 1.5rem;">
          <h1 style="font-size: 1.75rem; font-weight: 700;">Ask Calls AI</h1>
          <p style="color: var(--text-secondary); font-size: 0.9rem;">Ask natural language questions across your entire call center audio archives.</p>
        </div>

        <div class="card">
          <form method="GET" action="/ask" style="display:flex; flex-direction:column; gap: 1rem;">
            <input type="text" name="q" value="{query_val}" placeholder="e.g. Why are customers requesting subscription cancellations this month?" style="background: var(--bg-primary); border: 1px solid var(--border-color); color: white; padding: 0.75rem 1rem; border-radius: 0.35rem; font-size: 1rem;">
            <div style="display:flex; justify-content:flex-end;">
              <button type="submit" style="background: var(--accent-blue); color: white; border: none; padding: 0.6rem 1.5rem; border-radius: 0.25rem; cursor: pointer; font-weight: 600; font-size: 0.95rem;">Ask AI</button>
            </div>
          </form>
        </div>

        {response_html}
        "#
    );

    render_layout("Ask AI", "ask", &body)
}
