use crate::views::layout::render_layout;
use callmind_core::Call;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// Call row item with detected language metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallListItem {
    pub call: Call,
    pub language: Option<String>,
}

/// Pagination metadata for list views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaginationInfo {
    pub current_page: u32,
    pub page_size: u32,
    pub total_count: u64,
    pub total_pages: u32,
    pub status_filter: Option<String>,
    pub language_filter: Option<String>,
    pub date_filter: Option<String>,
    pub query_search: Option<String>,
}

impl Default for PaginationInfo {
    fn default() -> Self {
        Self {
            current_page: 1,
            page_size: 25,
            total_count: 0,
            total_pages: 1,
            status_filter: None,
            language_filter: None,
            date_filter: None,
            query_search: None,
        }
    }
}

/// Helper function to percent-encode URL parameters safely.
fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{:02X}", b);
        }
    }
    out
}

/// Helper function to safely escape HTML and highlight matching search keywords.
fn highlight_text(text: &str, query: Option<&str>) -> String {
    let escaped = html_escape::encode_text(text).to_string();
    if let Some(q) = query.filter(|s| !s.trim().is_empty()) {
        let q_trimmed = q.trim();
        let q_escaped = html_escape::encode_text(q_trimmed).to_string();

        let lower_escaped = escaped.to_lowercase();
        let lower_q = q_escaped.to_lowercase();

        if let Some(pos) = lower_escaped.find(&lower_q) {
            if escaped.is_char_boundary(pos) && escaped.is_char_boundary(pos + q_escaped.len()) {
                let before = &escaped[..pos];
                let matched = &escaped[pos..pos + q_escaped.len()];
                let after = &escaped[pos + q_escaped.len()..];
                return format!(
                    "{before}<mark style=\"background:#fbbf24; color:#0f172a; padding:0.05rem 0.2rem; border-radius:0.2rem; font-weight:600;\">{matched}</mark>{after}"
                );
            }
        }
    }
    escaped
}

/// Render the Calls List page HTML with pagination controls.
#[must_use]
pub fn render_calls_list(items: &[CallListItem], pagination: &PaginationInfo) -> String {
    let search_val = html_escape::encode_text(pagination.query_search.as_deref().unwrap_or(""));
    let current_status = pagination.status_filter.as_deref().unwrap_or("all");
    let current_lang = pagination.language_filter.as_deref().unwrap_or("all");
    let current_date = pagination.date_filter.as_deref().unwrap_or("all");

    let mut rows_html = String::new();

    if items.is_empty() {
        rows_html.push_str(
            r#"<tr><td colspan="8" style="text-align:center; padding: 2.5rem; color: var(--text-muted);">No calls found for this page/filter. Upload a recording or import audio files to begin.</td></tr>"#
        );
    } else {
        for item in items {
            let call = &item.call;
            let duration_str = call.duration_ms.map_or_else(
                || "—".to_string(),
                |ms| {
                    let secs = ms / 1000;
                    format!("{:02}:{:02}", secs / 60, secs % 60)
                },
            );

            let started_str = call.started_at.map_or_else(
                || call.created_at.format("%Y-%m-%d %H:%M").to_string(),
                |dt| dt.format("%Y-%m-%d %H:%M").to_string(),
            );

            // Who was on the call. A voice note or an uploaded file has no
            // numbers at all, and "Unknown -> Unknown" is noise rather than
            // information -- an em dash says the same thing without pretending
            // two sides were identified.
            let parties = match (call.phone_from.as_deref(), call.phone_to.as_deref()) {
                (None, None) => "&mdash;".to_string(),
                (Some(from), None) => html_escape::encode_text(from).to_string(),
                (None, Some(to)) => {
                    format!("&rarr; {}", html_escape::encode_text(to))
                }
                (Some(from), Some(to)) => format!(
                    "{} &rarr; {}",
                    html_escape::encode_text(from),
                    html_escape::encode_text(to)
                ),
            };
            let status = html_escape::encode_text(call.processing_status.as_str());
            let status_badge_class = if status == "completed" {
                "badge-completed"
            } else {
                "badge-pending"
            };

            let lang_str = item.language.as_deref().unwrap_or("").to_lowercase();
            let (badge_class, badge_label) = match lang_str.as_str() {
                "hebrew" | "he" => ("badge-he", "HE"),
                "russian" | "ru" => ("badge-ru", "RU"),
                "english" | "en" => ("badge-en", "EN"),
                "mixed" | "mix" => ("badge-mix", "MIX"),
                "pl" | "polish" => ("badge-en", "PL"),
                "es" | "spanish" => ("badge-en", "ES"),
                "ar" | "arabic" => ("badge-he", "AR"),
                _ => ("badge-pending", "—"),
            };

            let star_icon = if call.is_favorite { "★" } else { "☆" };
            let star_color = if call.is_favorite {
                "#fbbf24"
            } else {
                "var(--text-muted)"
            };

            let id_short = call.id.to_string();
            let raw_ext_id = call.external_id.as_deref().unwrap_or(&id_short[..8]);
            let highlighted_ext_id = highlight_text(raw_ext_id, pagination.query_search.as_deref());

            let _ = write!(
                rows_html,
                r#"<tr id="row-{id}">
                  <td>
                    <div style="display:flex; align-items:center; gap:0.4rem;">
                      <input type="checkbox" class="call-select-box" value="{id}" onchange="updateSelectedCount()">
                      <span id="star-{id}" onclick="toggleFavorite(event, '{id}')" style="cursor:pointer; color:{star_color}; font-size:1.1rem; line-height:1;" title="Toggle favorite">{star_icon}</span>
                    </div>
                  </td>
                  <td><a href="/calls/{id}" style="color: #60a5fa; font-weight:600; font-family:monospace;">{highlighted_ext_id}</a></td>
                  <td>{started_str}</td>
                  <td><span class="badge {badge_class}">{badge_label}</span></td>
                  <td>{duration_str}</td>
                  <td>{parties}</td>
                  <td><span class="badge {status_badge_class}">{status}</span></td>
                  <td>
                    <div style="display:flex; align-items:center; gap:0.4rem;">
                      <a href="/calls/{id}" style="color: #93c5fd; text-decoration: underline; font-size:0.85rem;">View &rarr;</a>
                      <button id="reprocess-btn-{id}" onclick="reprocessSingleCall(event, '{id}')" style="background:rgba(245,158,11,0.15); border:1px solid rgba(245,158,11,0.3); color:#fbbf24; padding:0.2rem 0.45rem; border-radius:0.25rem; font-size:0.75rem; cursor:pointer;" title="Re-analyze this conversation">🔄</button>
                      <button onclick="deleteSingleCall(event, '{id}')" style="background:rgba(239,68,68,0.15); border:1px solid rgba(239,68,68,0.3); color:#f87171; padding:0.2rem 0.45rem; border-radius:0.25rem; font-size:0.75rem; cursor:pointer;" title="Delete this conversation">🗑️</button>
                    </div>
                  </td>
                </tr>"#,
                id = call.id,
                star_icon = star_icon,
                star_color = star_color,
                highlighted_ext_id = highlighted_ext_id,
                started_str = started_str,
                badge_class = badge_class,
                badge_label = badge_label,
                duration_str = duration_str,
                parties = parties,
                status = status,
                status_badge_class = status_badge_class
            );
        }
    }

    // Filter query string builder helper
    let build_filter_url = |st: &str, lang: &str, date: &str| {
        let mut parts = vec![format!("page_size={}", pagination.page_size)];
        if st != "all" {
            parts.push(format!("status={}", url_encode(st)));
        }
        if lang != "all" {
            parts.push(format!("language={}", url_encode(lang)));
        }
        if date != "all" {
            parts.push(format!("date={}", url_encode(date)));
        }
        if let Some(ref q) = pagination.query_search {
            if !q.is_empty() {
                parts.push(format!("q={}", url_encode(q)));
            }
        }
        format!("/calls?{}", parts.join("&"))
    };

    // Status tabs
    let make_status_tab = |name: &str, val: &str| {
        let active = if current_status == val {
            "background: var(--accent-blue); color: white; font-weight:600;"
        } else {
            "background: var(--bg-card); color: var(--text-secondary);"
        };
        let url = build_filter_url(val, current_lang, current_date);
        format!(
            r#"<a href="{url}" style="padding: 0.35rem 0.85rem; border-radius: 0.25rem; font-size: 0.85rem; border: 1px solid var(--border-color); {active}">{name}</a>"#
        )
    };

    // Language pills
    let make_lang_pill = |name: &str, val: &str| {
        let active = if current_lang == val {
            "background: #4f46e5; color: white; font-weight:600;"
        } else {
            "background: rgba(255,255,255,0.05); color: var(--text-secondary);"
        };
        let url = build_filter_url(current_status, val, current_date);
        format!(
            r#"<a href="{url}" style="padding: 0.25rem 0.65rem; border-radius: 1rem; font-size: 0.75rem; border: 1px solid var(--border-color); {active}">{name}</a>"#
        )
    };

    // Date pills
    let make_date_pill = |name: &str, val: &str| {
        let active = if current_date == val {
            "background: #059669; color: white; font-weight:600;"
        } else {
            "background: rgba(255,255,255,0.05); color: var(--text-secondary);"
        };
        let url = build_filter_url(current_status, current_lang, val);
        format!(
            r#"<a href="{url}" style="padding: 0.25rem 0.65rem; border-radius: 1rem; font-size: 0.75rem; border: 1px solid var(--border-color); {active}">{name}</a>"#
        )
    };

    let filter_bar = format!(
        r#"
        <div style="display:flex; flex-direction:column; gap:0.75rem; margin-bottom:1.25rem;">
          <!-- Status Tabs -->
          <div style="display:flex; justify-content:space-between; align-items:center; flex-wrap:wrap; gap:0.75rem;">
            <div style="display:flex; gap:0.5rem;">
              {}
              {}
              {}
              {}
            </div>
            <!-- Language Filters -->
            <div style="display:flex; align-items:center; gap:0.4rem;">
              <span style="font-size:0.75rem; color:var(--text-muted); text-transform:uppercase;">Language:</span>
              {}
              {}
              {}
              {}
            </div>
          </div>
          <!-- Date Filter Pills -->
          <div style="display:flex; align-items:center; gap:0.4rem;">
            <span style="font-size:0.75rem; color:var(--text-muted); text-transform:uppercase;">Date:</span>
            {}
            {}
            {}
            {}
          </div>
        </div>
        "#,
        make_status_tab("All", "all"),
        make_status_tab("Completed", "completed"),
        make_status_tab("Pending", "pending"),
        make_status_tab("Processing", "processing"),
        make_lang_pill("All", "all"),
        make_lang_pill("Hebrew (עברית)", "he"),
        make_lang_pill("Russian (Русский)", "ru"),
        make_lang_pill("English", "en"),
        make_date_pill("All Time", "all"),
        make_date_pill("Today", "today"),
        make_date_pill("Past 7 Days", "7d"),
        make_date_pill("Past 30 Days", "30d"),
    );

    // Pagination bottom bar
    let cur_p = pagination.current_page;
    let total_p = pagination.total_pages.max(1);
    let ps = pagination.page_size;

    let build_page_url = |p: u32, size: u32| {
        let mut parts = vec![format!("page={p}"), format!("page_size={size}")];
        if current_status != "all" {
            parts.push(format!("status={}", url_encode(current_status)));
        }
        if current_lang != "all" {
            parts.push(format!("language={}", url_encode(current_lang)));
        }
        if current_date != "all" {
            parts.push(format!("date={}", url_encode(current_date)));
        }
        if let Some(ref q) = pagination.query_search {
            if !q.is_empty() {
                parts.push(format!("q={}", url_encode(q)));
            }
        }
        format!("/calls?{}", parts.join("&"))
    };

    let start_item = if pagination.total_count == 0 {
        0
    } else {
        (cur_p.saturating_sub(1) * ps) + 1
    };
    let end_item = ((cur_p * ps) as u64).min(pagination.total_count);

    let prev_link = if cur_p > 1 {
        let url = build_page_url(cur_p - 1, ps);
        format!(
            r#"<a href="{url}" style="padding:0.4rem 0.8rem; background:var(--bg-card); border:1px solid var(--border-color); border-radius:0.25rem; color:white;">&laquo; Prev</a>"#
        )
    } else {
        r#"<span style="padding:0.4rem 0.8rem; background:rgba(255,255,255,0.05); border:1px solid var(--border-color); border-radius:0.25rem; color:var(--text-muted); cursor:not-allowed;">&laquo; Prev</span>"#.to_string()
    };

    let next_link = if cur_p < total_p {
        let url = build_page_url(cur_p + 1, ps);
        format!(
            r#"<a href="{url}" style="padding:0.4rem 0.8rem; background:var(--bg-card); border:1px solid var(--border-color); border-radius:0.25rem; color:white;">Next &raquo;</a>"#
        )
    } else {
        r#"<span style="padding:0.4rem 0.8rem; background:rgba(255,255,255,0.05); border:1px solid var(--border-color); border-radius:0.25rem; color:var(--text-muted); cursor:not-allowed;">Next &raquo;</span>"#.to_string()
    };

    let mut page_numbers_html = String::new();
    let start_page = cur_p.saturating_sub(2).max(1);
    let end_page = (cur_p + 2).min(total_p);

    for p in start_page..=end_page {
        let active_style = if p == cur_p {
            "background:var(--accent-blue); color:white; font-weight:700;"
        } else {
            "background:var(--bg-card); color:var(--text-secondary);"
        };
        let url = build_page_url(p, ps);
        let _ = write!(
            page_numbers_html,
            r#"<a href="{url}" style="padding:0.35rem 0.65rem; border:1px solid var(--border-color); border-radius:0.25rem; font-size:0.85rem; {active_style}">{p}</a>"#
        );
    }

    let url_25 = build_page_url(1, 25);
    let url_50 = build_page_url(1, 50);
    let url_100 = build_page_url(1, 100);

    let pagination_controls = format!(
        r#"
        <div style="display:flex; justify-content:space-between; align-items:center; margin-top: 1.25rem; font-size:0.85rem; color:var(--text-secondary);">
          <div>Showing <strong>{start_item}–{end_item}</strong> of <strong>{}</strong> calls</div>
          <div style="display:flex; align-items:center; gap:0.4rem;">
            {prev_link}
            {page_numbers_html}
            {next_link}
          </div>
          <div style="display:flex; align-items:center; gap:0.4rem;">
            <span>Per page:</span>
            <a href="{url_25}" style="padding:0.2rem 0.4rem; border-radius:0.2rem; {ps25}">25</a>
            <a href="{url_50}" style="padding:0.2rem 0.4rem; border-radius:0.2rem; {ps50}">50</a>
            <a href="{url_100}" style="padding:0.2rem 0.4rem; border-radius:0.2rem; {ps100}">100</a>
          </div>
        </div>
        "#,
        pagination.total_count,
        ps25 = if ps == 25 {
            "background:var(--accent-blue); color:white;"
        } else {
            "color:var(--text-secondary);"
        },
        ps50 = if ps == 50 {
            "background:var(--accent-blue); color:white;"
        } else {
            "color:var(--text-secondary);"
        },
        ps100 = if ps == 100 {
            "background:var(--accent-blue); color:white;"
        } else {
            "color:var(--text-secondary);"
        }
    );

    let body = format!(
        r#"
        <div style="display:flex; justify-content:space-between; align-items:center; margin-bottom: 1.5rem; flex-wrap:wrap; gap:1rem;">
          <div>
            <h1 style="font-size: 1.75rem; font-weight: 700;">Recorded Conversations</h1>
            <p style="color: var(--text-secondary); font-size: 0.9rem;">Inspect transcripts, listen to audio, and monitor background processing.</p>
          </div>
          <div style="display:flex; align-items:center; gap:0.75rem; flex-wrap:wrap;">
            <button id="reanalyze-selected-btn" onclick="reanalyzeSelected()" style="display:none; align-items:center; gap:0.4rem; background:linear-gradient(135deg, #f59e0b, #d97706); color:white; border:none; padding:0.55rem 1rem; border-radius:0.35rem; cursor:pointer; font-weight:600; font-size:0.85rem; box-shadow:0 2px 8px rgba(245,158,11,0.3);">
              <span>⚡ Re-analyze Selected (<span id="selected-count">0</span>)</span>
            </button>
            <button id="delete-selected-btn" onclick="deleteSelected()" style="display:none; align-items:center; gap:0.4rem; background:linear-gradient(135deg, #ef4444, #b91c1c); color:white; border:none; padding:0.55rem 1rem; border-radius:0.35rem; cursor:pointer; font-weight:600; font-size:0.85rem; box-shadow:0 2px 8px rgba(239,68,68,0.3);">
              <span>🗑️ Delete Selected (<span id="delete-selected-count">0</span>)</span>
            </button>
            <button onclick="reanalyzeAll()" style="display:inline-flex; align-items:center; gap:0.4rem; background:rgba(245,158,11,0.15); border:1px solid rgba(245,158,11,0.3); color:#fbbf24; padding:0.55rem 1rem; border-radius:0.35rem; cursor:pointer; font-weight:600; font-size:0.85rem;">
              <span>🔄 Re-analyze All</span>
            </button>
            <button onclick="openUploadModal()" style="display:inline-flex; align-items:center; gap:0.4rem; background: linear-gradient(135deg, #3b82f6, #6366f1); color: white; border: none; padding: 0.55rem 1.1rem; border-radius: 0.35rem; cursor: pointer; font-weight: 600; font-size:0.9rem; box-shadow: 0 2px 8px rgba(59,130,246,0.35);">
              <span>📤 Upload Audio</span>
            </button>
            <form method="GET" action="/calls" style="display:flex; gap: 0.5rem;">
              <input type="text" name="q" value="{search_val}" placeholder="Search transcript or topics..." style="background: var(--bg-card); border: 1px solid var(--border-color); color: white; padding: 0.5rem 1rem; border-radius: 0.25rem; width: 260px;">
              <button type="submit" style="background: var(--accent-blue); color: white; border: none; padding: 0.5rem 1rem; border-radius: 0.25rem; cursor: pointer; font-weight: 600;">Search</button>
            </form>
          </div>
        </div>

        {filter_bar}

        <div class="table-container">
          <table>
            <thead>
              <tr>
                <th style="width: 40px;"><input type="checkbox" id="select-all-box" onchange="toggleSelectAll(this)" style="cursor:pointer;" title="Select All"></th>
                <th>Call ID</th>
                <th>Started Time</th>
                <th>Language</th>
                <th>Duration</th>
                <th>Parties</th>
                <th>Status</th>
                <th>Action</th>
              </tr>
            </thead>
            <tbody>
              {rows_html}
            </tbody>
          </table>
        </div>

        {pagination_controls}

        <!-- Upload & Record Modal -->
        <div id="upload-modal" style="display:none; position:fixed; inset:0; background:rgba(0,0,0,0.75); z-index:1000; align-items:center; justify-content:center; padding:1rem; backdrop-filter:blur(4px);">
          <div style="background:var(--bg-secondary); border:1px solid var(--border-color); border-radius:0.75rem; max-width:520px; width:100%; padding:1.75rem; box-shadow:0 20px 25px -5px rgba(0,0,0,0.5); position:relative;">
            <div style="display:flex; justify-content:space-between; align-items:center; margin-bottom:1rem;">
              <h3 style="font-size:1.25rem; font-weight:700;">Add Audio Recording</h3>
              <button onclick="closeUploadModal()" style="background:none; border:none; color:var(--text-secondary); font-size:1.5rem; cursor:pointer; line-height:1;">&times;</button>
            </div>

            <!-- Modal Tab Switcher -->
            <div style="display:flex; gap:0.5rem; margin-bottom:1.25rem; border-bottom:1px solid var(--border-color); padding-bottom:0.5rem;">
              <button id="tab-file-btn" onclick="switchModalTab('file')" style="background:var(--accent-blue); color:white; border:none; padding:0.35rem 0.85rem; border-radius:0.25rem; font-size:0.85rem; cursor:pointer; font-weight:600;">📁 Upload File</button>
              <button id="tab-mic-btn" onclick="switchModalTab('mic')" style="background:rgba(255,255,255,0.05); color:var(--text-secondary); border:1px solid var(--border-color); padding:0.35rem 0.85rem; border-radius:0.25rem; font-size:0.85rem; cursor:pointer;">🎙️ Record from Mic</button>
            </div>
            
            <!-- File Drop Zone -->
            <div id="file-tab-content">
              <div id="drop-zone" ondragover="handleDragOver(event)" ondragleave="handleDragLeave(event)" ondrop="handleDrop(event)" onclick="document.getElementById('file-input').click()" style="border:2px dashed var(--border-color); border-radius:0.5rem; padding:2.5rem 1.5rem; text-align:center; cursor:pointer; background:rgba(0,0,0,0.15); transition:all 0.2s ease;">
                <div style="font-size:2.5rem; margin-bottom:0.75rem;">📁</div>
                <p style="font-weight:600; margin-bottom:0.25rem;">Drag & drop your audio file here</p>
                <p style="font-size:0.8rem; color:var(--text-muted); margin-bottom:0.75rem;">Supports M4A, MP3, WAV, OGG, FLAC, AAC</p>
                <span style="display:inline-block; font-size:0.85rem; background:rgba(255,255,255,0.08); border:1px solid var(--border-color); padding:0.35rem 0.85rem; border-radius:0.25rem; color:var(--text-secondary);">or browse from computer</span>
                <input type="file" id="file-input" accept="audio/*,.m4a,.mp3,.wav,.ogg,.flac,.aac" style="display:none;" onchange="handleFileSelect(event)">
              </div>

              <div id="selected-file-info" style="display:none; margin-top:1rem; padding:0.75rem; background:rgba(59,130,246,0.1); border:1px solid rgba(59,130,246,0.3); border-radius:0.35rem; font-size:0.85rem;">
                <div style="display:flex; justify-content:space-between; align-items:center;">
                  <span id="selected-file-name" style="font-weight:600; color:white; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; max-width:350px;">filename.m4a</span>
                  <span id="selected-file-size" style="color:var(--text-secondary);">1.2 MB</span>
                </div>
              </div>
            </div>

            <!-- Mic Recording Content -->
            <div id="mic-tab-content" style="display:none; text-align:center; padding:1.5rem 0;">
              <div id="mic-pulse" style="width:72px; height:72px; border-radius:50%; background:rgba(239,68,68,0.2); border:2px solid var(--accent-red); margin:0 auto 1rem auto; display:flex; align-items:center; justify-content:center; font-size:2rem; cursor:pointer;" onclick="toggleMicRecording()">
                🎙️
              </div>
              <div id="rec-status-label" style="font-weight:600; font-size:1rem; margin-bottom:0.25rem;">Click to Start Recording</div>
              <div id="rec-timer" style="font-family:monospace; font-size:1.25rem; color:var(--text-secondary); margin-bottom:1rem;">00:00</div>
              <audio id="rec-preview-audio" controls style="display:none; width:100%; margin-top:0.5rem;"></audio>
            </div>

            <div id="upload-progress-container" style="display:none; margin-top:1rem;">
              <div style="display:flex; justify-content:space-between; font-size:0.8rem; margin-bottom:0.25rem;">
                <span id="upload-status-text">Uploading & Queuing...</span>
                <span id="upload-percent">0%</span>
              </div>
              <div style="background:rgba(255,255,255,0.1); height:8px; border-radius:4px; overflow:hidden;">
                <div id="upload-progress-bar" style="background:var(--accent-blue); width:0%; height:100%; transition:width 0.2s;"></div>
              </div>
            </div>

            <div style="display:flex; justify-content:flex-end; gap:0.75rem; margin-top:1.5rem;">
              <button onclick="closeUploadModal()" style="background:var(--bg-card); border:1px solid var(--border-color); color:var(--text-secondary); padding:0.5rem 1rem; border-radius:0.25rem; cursor:pointer;">Cancel</button>
              <button id="start-upload-btn" onclick="startUpload()" disabled style="background:var(--accent-blue); color:white; border:none; padding:0.5rem 1.25rem; border-radius:0.25rem; cursor:not-allowed; opacity:0.5; font-weight:600;">Start Processing</button>
            </div>
          </div>
        </div>

        <script>
          let currentFile = null;
          let audioCtx = null;
          let micStream = null;
          let scriptNode = null;
          let pcmData = [];
          let recInterval = null;
          let recSeconds = 0;
          let isRecording = false;

          function switchModalTab(tab) {{
            const fileTab = document.getElementById('file-tab-content');
            const micTab = document.getElementById('mic-tab-content');
            const fileBtn = document.getElementById('tab-file-btn');
            const micBtn = document.getElementById('tab-mic-btn');

            if (tab === 'mic') {{
              fileTab.style.display = 'none';
              micTab.style.display = 'block';
              micBtn.style.background = 'var(--accent-blue)';
              micBtn.style.color = 'white';
              fileBtn.style.background = 'rgba(255,255,255,0.05)';
              fileBtn.style.color = 'var(--text-secondary)';
            }} else {{
              fileTab.style.display = 'block';
              micTab.style.display = 'none';
              fileBtn.style.background = 'var(--accent-blue)';
              fileBtn.style.color = 'white';
              micBtn.style.background = 'rgba(255,255,255,0.05)';
              micBtn.style.color = 'var(--text-secondary)';
            }}
          }}

          async function toggleMicRecording() {{
            if (isRecording) {{
              // Stop Recording
              isRecording = false;
              clearInterval(recInterval);

              if (scriptNode) {{ scriptNode.disconnect(); scriptNode = null; }}
              if (micStream) {{ micStream.getTracks().forEach(t => t.stop()); micStream = null; }}
              if (audioCtx) {{ audioCtx.close(); audioCtx = null; }}

              document.getElementById('rec-status-label').innerText = 'Recording Finished ✓';
              document.getElementById('mic-pulse').style.background = 'rgba(16,185,129,0.2)';
              document.getElementById('mic-pulse').style.borderColor = 'var(--accent-green)';

              // Merge PCM float32 buffers
              let totalLength = pcmData.reduce((acc, chunk) => acc + chunk.length, 0);
              let merged = new Float32Array(totalLength);
              let offset = 0;
              for (let chunk of pcmData) {{
                merged.set(chunk, offset);
                offset += chunk.length;
              }}

              // Encode to 16kHz standard Linear PCM WAV
              const wavBlob = encodePcmWav(merged, 16000);
              const fileName = `mic_recording_${{new Date().toISOString().slice(0,19).replace(/[:T]/g,'_')}}.wav`;
              const file = new File([wavBlob], fileName, {{ type: 'audio/wav' }});
              setFile(file);

              const preview = document.getElementById('rec-preview-audio');
              preview.src = URL.createObjectURL(wavBlob);
              preview.style.display = 'block';
            }} else {{
              // Start Recording
              try {{
                micStream = await navigator.mediaDevices.getUserMedia({{ audio: {{ channelCount: 1, sampleRate: 16000 }} }});
                audioCtx = new (window.AudioContext || window.webkitAudioContext)({{ sampleRate: 16000 }});
                const source = audioCtx.createMediaStreamSource(micStream);

                scriptNode = audioCtx.createScriptProcessor(4096, 1, 1);
                pcmData = [];

                scriptNode.onaudioprocess = (e) => {{
                  if (isRecording) {{
                    const input = e.inputBuffer.getChannelData(0);
                    pcmData.push(new Float32Array(input));
                  }}
                }};

                source.connect(scriptNode);
                scriptNode.connect(audioCtx.destination);

                isRecording = true;
                recSeconds = 0;
                document.getElementById('rec-preview-audio').style.display = 'none';
                document.getElementById('rec-status-label').innerText = '🔴 Recording... Click to Stop';
                document.getElementById('mic-pulse').style.background = 'rgba(239,68,68,0.4)';
                document.getElementById('mic-pulse').style.borderColor = 'var(--accent-red)';
                document.getElementById('rec-timer').innerText = '00:00';

                recInterval = setInterval(() => {{
                  recSeconds++;
                  const m = String(Math.floor(recSeconds / 60)).padStart(2, '0');
                  const s = String(recSeconds % 60).padStart(2, '0');
                  document.getElementById('rec-timer').innerText = `${{m}}:${{s}}`;
                }}, 1000);
              }} catch (err) {{
                alert(`Microphone access error: ${{err.message}}`);
              }}
            }}
          }}

          function encodePcmWav(samples, sampleRate) {{
            const buffer = new ArrayBuffer(44 + samples.length * 2);
            const view = new DataView(buffer);

            // RIFF chunk descriptor
            writeString(view, 0, 'RIFF');
            view.setUint32(4, 36 + samples.length * 2, true);
            writeString(view, 8, 'WAVE');

            // fmt sub-chunk
            writeString(view, 12, 'fmt ');
            view.setUint32(16, 16, true);
            view.setUint16(20, 1, true); // Linear PCM
            view.setUint16(22, 1, true); // Mono
            view.setUint32(24, sampleRate, true);
            view.setUint32(28, sampleRate * 2, true); // byte rate (SampleRate * NumChannels * BitsPerSample/8)
            view.setUint16(32, 2, true); // block align
            view.setUint16(34, 16, true); // bits per sample

            // data sub-chunk
            writeString(view, 36, 'data');
            view.setUint32(40, samples.length * 2, true);

            // write 16-bit PCM samples
            let offset = 44;
            for (let i = 0; i < samples.length; i++, offset += 2) {{
              let s = Math.max(-1, Math.min(1, samples[i]));
              view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7FFF, true);
            }}

            return new Blob([view], {{ type: 'audio/wav' }});
          }}

          function writeString(view, offset, string) {{
            for (let i = 0; i < string.length; i++) {{
              view.setUint8(offset + i, string.charCodeAt(i));
            }}
          }}

          async function toggleFavorite(event, id) {{
            event.stopPropagation();
            const star = document.getElementById(`star-${{id}}`);
            try {{
              const res = await fetch(`/api/v1/calls/${{id}}/favorite`, {{ method: 'POST' }});
              if (res.ok) {{
                const data = await res.json();
                if (star) {{
                  star.innerText = data.is_favorite ? '★' : '☆';
                  star.style.color = data.is_favorite ? '#fbbf24' : 'var(--text-muted)';
                }}
              }}
            }} catch (e) {{}}
          }}

          function toggleSelectAll(masterBox) {{
            const boxes = document.querySelectorAll('.call-select-box');
            boxes.forEach(b => b.checked = masterBox.checked);
            updateSelectedCount();
          }}

          function updateSelectedCount() {{
            const selected = document.querySelectorAll('.call-select-box:checked');
            const count = selected.length;
            const reanalyzeBtn = document.getElementById('reanalyze-selected-btn');
            const deleteBtn = document.getElementById('delete-selected-btn');
            const countEl = document.getElementById('selected-count');
            const delCountEl = document.getElementById('delete-selected-count');

            if (countEl) countEl.innerText = count;
            if (delCountEl) delCountEl.innerText = count;

            if (reanalyzeBtn) reanalyzeBtn.style.display = count > 0 ? 'inline-flex' : 'none';
            if (deleteBtn) deleteBtn.style.display = count > 0 ? 'inline-flex' : 'none';
          }}

          async function deleteSingleCall(event, id) {{
            event.stopPropagation();
            if (!confirm('Are you sure you want to delete this conversation and its recording?')) return;
            try {{
              const res = await fetch(`/api/v1/calls/${{id}}`, {{ method: 'DELETE' }});
              if (res.ok || res.status === 204) {{
                const row = document.getElementById(`row-${{id}}`);
                if (row) row.remove();
                updateSelectedCount();
              }} else {{
                alert('Failed to delete conversation.');
              }}
            }} catch (err) {{
              alert(`Error: ${{err.message}}`);
            }}
          }}

          async function deleteSelected() {{
            const selected = Array.from(document.querySelectorAll('.call-select-box:checked')).map(b => b.value);
            if (selected.length === 0) return;
            if (!confirm(`Permanently delete ${{selected.length}} selected conversations and recordings?`)) return;

            for (const id of selected) {{
              try {{
                await fetch(`/api/v1/calls/${{id}}`, {{ method: 'DELETE' }});
                const row = document.getElementById(`row-${{id}}`);
                if (row) row.remove();
              }} catch (e) {{}}
            }}
            updateSelectedCount();
            alert(`Deleted ${{selected.length}} conversations.`);
          }}

          async function reprocessSingleCall(event, id) {{
            event.stopPropagation();
            const btn = document.getElementById(`reprocess-btn-${{id}}`);
            if (btn) {{
              btn.disabled = true;
              btn.innerText = '⏳';
            }}
            try {{
              const res = await fetch(`/api/v1/calls/${{id}}/reprocess`, {{ method: 'POST' }});
              if (res.ok) {{
                if (btn) btn.innerText = '✓';
                setTimeout(() => {{
                  if (btn) {{
                    btn.disabled = false;
                    btn.innerText = '🔄';
                  }}
                }}, 3000);
              }} else {{
                alert('Failed to queue reprocess');
                if (btn) {{
                  btn.disabled = false;
                  btn.innerText = '🔄';
                }}
              }}
            }} catch (err) {{
              alert(`Error: ${{err.message}}`);
              if (btn) {{
                btn.disabled = false;
                btn.innerText = '🔄';
              }}
            }}
          }}

          async function reanalyzeSelected() {{
            const selected = Array.from(document.querySelectorAll('.call-select-box:checked')).map(b => b.value);
            if (selected.length === 0) return;
            if (!confirm(`Re-analyze ${{selected.length}} selected conversations with Ollama AI?`)) return;

            for (const id of selected) {{
              try {{
                await fetch(`/api/v1/calls/${{id}}/reprocess`, {{ method: 'POST' }});
              }} catch (e) {{}}
            }}

            alert(`Queued ${{selected.length}} conversations for re-analysis! Check navbar for progress.`);
            setTimeout(() => window.location.reload(), 1500);
          }}

          async function reanalyzeAll() {{
            if (!confirm('Re-run AI analysis for all conversations using Ollama?')) return;
            try {{
              const res = await fetch('/api/v1/calls/reanalyze-all', {{ method: 'POST' }});
              if (res.ok) {{
                const data = await res.json();
                alert(`Queued ${{data.count}} conversations for AI re-analysis! Progress will update in the top navbar.`);
                setTimeout(() => window.location.reload(), 1500);
              }} else {{
                alert('Failed to trigger re-analysis.');
              }}
            }} catch (err) {{
              alert(`Error: ${{err.message}}`);
            }}
          }}

          function openUploadModal() {{
            document.getElementById('upload-modal').style.display = 'flex';
          }}

          function closeUploadModal() {{
            document.getElementById('upload-modal').style.display = 'none';
            currentFile = null;
            document.getElementById('selected-file-info').style.display = 'none';
            document.getElementById('upload-progress-container').style.display = 'none';
            const btn = document.getElementById('start-upload-btn');
            btn.disabled = true;
            btn.style.opacity = '0.5';
            btn.style.cursor = 'not-allowed';
          }}

          function handleDragOver(e) {{
            e.preventDefault();
            e.stopPropagation();
            document.getElementById('drop-zone').style.borderColor = 'var(--accent-blue)';
            document.getElementById('drop-zone').style.background = 'rgba(59,130,246,0.1)';
          }}

          function handleDragLeave(e) {{
            e.preventDefault();
            e.stopPropagation();
            document.getElementById('drop-zone').style.borderColor = 'var(--border-color)';
            document.getElementById('drop-zone').style.background = 'rgba(0,0,0,0.15)';
          }}

          function handleDrop(e) {{
            e.preventDefault();
            e.stopPropagation();
            handleDragLeave(e);
            if (e.dataTransfer.files && e.dataTransfer.files.length > 0) {{
              setFile(e.dataTransfer.files[0]);
            }}
          }}

          function handleFileSelect(e) {{
            if (e.target.files && e.target.files.length > 0) {{
              setFile(e.target.files[0]);
            }}
          }}

          function setFile(file) {{
            currentFile = file;
            document.getElementById('selected-file-name').innerText = file.name;
            const sizeMb = (file.size / (1024 * 1024)).toFixed(2);
            document.getElementById('selected-file-size').innerText = `${{sizeMb}} MB`;
            document.getElementById('selected-file-info').style.display = 'block';

            const btn = document.getElementById('start-upload-btn');
            btn.disabled = false;
            btn.style.opacity = '1';
            btn.style.cursor = 'pointer';
          }}

          async function startUpload() {{
            if (!currentFile) return;

            const progressContainer = document.getElementById('upload-progress-container');
            const progressBar = document.getElementById('upload-progress-bar');
            const percentText = document.getElementById('upload-percent');
            const statusText = document.getElementById('upload-status-text');
            const startBtn = document.getElementById('start-upload-btn');

            progressContainer.style.display = 'block';
            startBtn.disabled = true;
            startBtn.style.opacity = '0.5';

            try {{
              // 1. Create Call
              statusText.innerText = 'Creating recording session...';
              progressBar.style.width = '20%';
              percentText.innerText = '20%';

              const createRes = await fetch('/api/v1/calls', {{
                method: 'POST',
                headers: {{ 'Content-Type': 'application/json' }},
                body: JSON.stringify({{ external_id: currentFile.name }})
              }});

              if (!createRes.ok) throw new Error('Failed to create call session');
              const callData = await createRes.json();
              const callId = callData.id;

              // 2. Upload Audio File
              statusText.innerText = 'Uploading audio file...';
              progressBar.style.width = '50%';
              percentText.innerText = '50%';

              const mimeType = currentFile.type || 'audio/m4a';
              const uploadRes = await fetch(`/api/v1/calls/${{callId}}/recording`, {{
                method: 'POST',
                headers: {{ 'Content-Type': mimeType }},
                body: currentFile
              }});

              if (!uploadRes.ok) throw new Error('Audio upload failed');

              progressBar.style.width = '100%';
              percentText.innerText = '100%';
              statusText.innerText = 'Done! Redirecting to call...';

              setTimeout(() => {{
                window.location.href = `/calls/${{callId}}`;
              }}, 600);
            }} catch (err) {{
              console.error(err);
              statusText.innerText = `Error: ${{err.message}}`;
              statusText.style.color = '#ef4444';
            }}
          }}
        </script>
        "#
    );

    render_layout("Calls", "calls", &body)
}
