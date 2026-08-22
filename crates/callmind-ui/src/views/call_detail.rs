use crate::views::layout::render_layout;
use callmind_analysis::CallAnalysis;
use callmind_core::Call;
use callmind_transcript::{TextDirection, Transcript};
use std::fmt::Write;

/// Render the Call Detail view HTML.
#[must_use]
pub fn render_call_detail(
    call: &Call,
    transcript: Option<&Transcript>,
    analysis: Option<&CallAnalysis>,
    last_error: Option<&str>,
) -> String {
    let call_id = call.id;
    let title = analysis.map_or_else(|| "Call Detail".to_string(), |a| a.title.clone());

    let is_failed =
        call.processing_status == callmind_core::ProcessingStatus::Failed || last_error.is_some();

    // Error banner if processing failed
    let error_banner = if let Some(err) = last_error {
        let escaped_err = html_escape::encode_text(err);
        format!(
            r#"
            <div style="background: rgba(239, 68, 68, 0.15); border: 1px solid var(--accent-red); border-radius: 0.5rem; padding: 1.25rem; margin-bottom: 1.5rem;">
              <div style="display:flex; align-items:center; gap:0.5rem; margin-bottom:0.5rem;">
                <span style="font-size:1.25rem;">⚠️</span>
                <strong style="color: #fca5a5; font-size: 0.95rem;">Processing Failed</strong>
              </div>
              <p style="color: #fca5a5; font-family: monospace; font-size: 0.85rem; background: rgba(0,0,0,0.3); padding: 0.6rem 0.8rem; border-radius: 0.25rem; margin-bottom: 0.75rem; word-break: break-word;">
                {escaped_err}
              </p>
              <button onclick="reprocessCall()" style="background: var(--accent-blue); color: white; border: none; padding: 0.4rem 1rem; border-radius: 0.25rem; font-size: 0.85rem; cursor: pointer; font-weight: 600;">
                🔄 Re-try Processing
              </button>
            </div>
            "#
        )
    } else {
        String::new()
    };

    // Left Panel: Analysis Information
    let summary_html = analysis.map_or_else(
        || {
            if is_failed {
                "<p style='color: #fca5a5;'>Processing failed. Check the error message above.</p>".to_string()
            } else {
                "<p style='color: var(--text-muted);'>⏳ Analysis processing in progress...</p>".to_string()
            }
        },
        |a| {
            let escaped_summary = html_escape::encode_text(&a.summary);
            let mut html = format!(
                r#"
                <div style="margin-bottom: 1.5rem;">
                  <h3 style="font-size: 0.9rem; color: var(--text-secondary); text-transform: uppercase; margin-bottom: 0.5rem;">Summary</h3>
                  <p style="font-size: 0.95rem; line-height: 1.6;">{escaped_summary}</p>
                </div>
                "#
            );

            if let Some(ref reason) = a.reason {
                let escaped_reason = html_escape::encode_text(reason);
                let _ = write!(
                    html,
                    r#"<div style="margin-bottom: 1rem;"><strong style="color: var(--text-secondary); font-size: 0.85rem;">Topic / Reason:</strong> <span style="font-size: 0.95rem;">{escaped_reason}</span></div>"#
                );
            }

            if let Some(ref res) = a.resolution {
                let escaped_res = html_escape::encode_text(res);
                let resolved_badge = if a.resolved {
                    "<span class='badge badge-completed'>Completed ✓</span>"
                } else {
                    "<span class='badge badge-pending'>Pending ✗</span>"
                };
                let _ = write!(
                    html,
                    r#"<div style="margin-bottom: 1.25rem;"><strong style="color: var(--text-secondary); font-size: 0.85rem;">Conclusion / Outcome:</strong> {resolved_badge} <span style="font-size: 0.95rem;">{escaped_res}</span></div>"#
                );
            }

            // Key Facts & Details
            if !a.key_facts.is_empty() {
                html.push_str(r#"<div style="margin-bottom: 1.25rem;"><h3 style="font-size: 0.9rem; color: var(--text-secondary); text-transform: uppercase; margin-bottom: 0.5rem;">📌 Key Facts & Details</h3><ul style="padding-left: 1.2rem; font-size: 0.9rem; line-height: 1.6;">"#);
                for fact in &a.key_facts {
                    let escaped_fact = html_escape::encode_text(fact);
                    let _ = write!(html, "<li style='margin-bottom: 0.25rem;'>{escaped_fact}</li>");
                }
                html.push_str("</ul></div>");
            }

            // Extracted Entities (People, Locations, Rooms, Phones, Amounts)
            if !a.entities.is_empty() {
                html.push_str(r#"<div style="margin-bottom: 1.25rem;"><h3 style="font-size: 0.9rem; color: var(--text-secondary); text-transform: uppercase; margin-bottom: 0.5rem;">🏷️ Extracted Entities</h3><div style="display:flex; flex-wrap:wrap; gap:0.4rem;">"#);
                for entity in &a.entities {
                    let escaped_val = html_escape::encode_text(&entity.value);
                    let escaped_type = html_escape::encode_text(&entity.entity_type);
                    let icon = match entity.entity_type.to_lowercase().as_str() {
                        t if t.contains("loc") || t.contains("place") || t.contains("address") || t.contains("floor") || t.contains("room") => "📍",
                        t if t.contains("person") || t.contains("name") => "👤",
                        t if t.contains("phone") => "📞",
                        t if t.contains("date") || t.contains("time") => "🕒",
                        t if t.contains("price") || t.contains("amount") || t.contains("money") => "💰",
                        _ => "🏷️",
                    };
                    let _ = write!(
                        html,
                        r#"<span style="background:rgba(59,130,246,0.15); border:1px solid rgba(59,130,246,0.3); color:#93c5fd; padding:0.2rem 0.6rem; border-radius:0.35rem; font-size:0.8rem;">{icon} <strong style="color:white;">{escaped_val}</strong> <span style="font-size:0.7rem; opacity:0.75;">({escaped_type})</span></span>"#
                    );
                }
                html.push_str("</div></div>");
            }

            // Topics & Intent
            if !a.topics.is_empty() {
                html.push_str(r#"<div style="margin-bottom: 1.25rem;"><h3 style="font-size: 0.9rem; color: var(--text-secondary); text-transform: uppercase; margin-bottom: 0.5rem;">🏷️ Topics & Intent</h3><div style="display:flex; flex-wrap:wrap; gap:0.35rem;">"#);
                if let Some(ref intent) = a.customer_intent {
                    let escaped_intent = html_escape::encode_text(intent);
                    let _ = write!(html, r#"<span style="background:rgba(16,185,129,0.15); border:1px solid rgba(16,185,129,0.3); color:#6ee7b7; padding:0.2rem 0.5rem; border-radius:0.25rem; font-size:0.8rem; font-weight:600;">🎯 {escaped_intent}</span>"#);
                }
                for topic in &a.topics {
                    let escaped_topic = html_escape::encode_text(&topic.name);
                    let _ = write!(html, r#"<span style="background:rgba(255,255,255,0.06); border:1px solid var(--border-color); color:var(--text-secondary); padding:0.2rem 0.5rem; border-radius:0.25rem; font-size:0.8rem;">#{escaped_topic}</span>"#);
                }
                html.push_str("</div></div>");
            }

            // Action Items & Grocery / To-Do List
            if !a.action_items.is_empty() {
                let mut todo_raw = String::new();
                for item in &a.action_items {
                    let _ = writeln!(todo_raw, "- [ ] {}", item.text);
                }
                let escaped_todo_js = html_escape::encode_text(&todo_raw);
                let encoded_whatsapp = urlencoding_simple(&format!("*To-Do List ({})*:\n{}", a.title, todo_raw));

                let _ = write!(
                    html,
                    r#"
                    <div style="margin-top: 1.5rem; background: rgba(0,0,0,0.15); border: 1px solid var(--border-color); border-radius: 0.5rem; padding: 1rem;">
                      <div style="display:flex; justify-content:space-between; align-items:center; margin-bottom: 0.75rem; flex-wrap:wrap; gap:0.5rem;">
                        <h3 style="font-size: 0.9rem; color: var(--text-secondary); text-transform: uppercase; margin:0;">📝 Smart To-Do & Grocery List</h3>
                        <div style="display:flex; gap:0.35rem;">
                          <button onclick="navigator.clipboard.writeText('{escaped_todo_js}'); showToast('📋 Tasks copied to clipboard!');" style="background:rgba(255,255,255,0.08); border:1px solid var(--border-color); color:white; padding:0.25rem 0.6rem; border-radius:0.25rem; font-size:0.75rem; cursor:pointer;" title="Copy as Markdown / Apple Reminders">📋 Copy Tasks</button>
                          <a href="https://api.whatsapp.com/send?text={encoded_whatsapp}" target="_blank" style="background:rgba(37,211,102,0.15); border:1px solid rgba(37,211,102,0.3); color:#86efac; padding:0.25rem 0.6rem; border-radius:0.25rem; font-size:0.75rem; text-decoration:none; font-weight:600; display:inline-flex; align-items:center; gap:0.25rem;">💬 WhatsApp</a>
                        </div>
                      </div>
                      <div style="display:flex; flex-direction:column; gap:0.4rem;">
                    "#
                );

                for item in &a.action_items {
                    let escaped_item = html_escape::encode_text(&item.text);
                    let owner_str = item.owner.map_or_else(String::new, |o| format!(" <span style='font-size:0.75rem; color:var(--text-muted); background:rgba(255,255,255,0.05); padding:0.1rem 0.4rem; border-radius:0.2rem;'>[{}]</span>", o.display_label(None)));
                    let deadline_str = item.deadline.as_deref().map_or_else(String::new, |d| format!(" <span style='font-size:0.75rem; color:#fbbf24;'>⏰ {}</span>", html_escape::encode_text(d)));

                    let _ = write!(
                        html,
                        r#"<label style="display:flex; align-items:flex-start; gap:0.5rem; font-size:0.9rem; cursor:pointer; line-height:1.4;">
                             <input type="checkbox" style="margin-top:0.2rem; cursor:pointer;">
                             <span><strong>{escaped_item}</strong>{owner_str}{deadline_str}</span>
                           </label>"#
                    );
                }
                html.push_str("</div></div>");
            }

            // Conversation Score / Rating
            if let Some(ref score) = a.scorecard {
                let _ = write!(
                    html,
                    r#"
                    <div style="margin-top: 1.5rem; background: rgba(0,0,0,0.2); padding: 1rem; border-radius: 0.5rem; border: 1px solid var(--border-color);">
                      <div style="display:flex; justify-content:space-between; align-items:center; margin-bottom: 0.5rem;">
                        <span style="font-size: 0.85rem; color: var(--text-secondary); text-transform: uppercase; font-weight:600;">Overall Rating</span>
                        <span style="font-size: 1.4rem; font-weight:700; color: #34d399;">{}/100</span>
                      </div>
                    </div>
                    "#,
                    score.total_score
                );
            }

            html
        },
    );

    // Right Panel: Transcript turns with RTL / LTR formatting and seek handlers
    let mut transcript_html = String::new();

    if let Some(t) = transcript {
        if t.segments.is_empty() {
            transcript_html.push_str("<p style='color: var(--text-muted); padding: 2rem; text-align:center;'>No speech detected in this audio recording.</p>");
        } else {
            for (idx, seg) in t.segments.iter().enumerate() {
                let start_sec = seg.start_ms as f64 / 1000.0;
                let end_sec = seg.end_ms as f64 / 1000.0;
                let time_str = format!(
                    "{:02}:{:02}",
                    (seg.start_ms / 60000),
                    (seg.start_ms % 60000) / 1000
                );

                let speaker_raw = seg
                    .speaker_role
                    .display_label(Some(seg.speaker_id.as_u16()));
                let speaker_name = html_escape::encode_text(&speaker_raw);
                let speaker_class = format!("turn-speaker-{}", (seg.speaker_id.as_u16() % 4) + 1);

                let dir_class = match seg.text_direction {
                    TextDirection::Rtl => "dir-rtl",
                    TextDirection::Ltr => "dir-ltr",
                };

                let lang_code = seg.language.code().to_uppercase();
                let escaped_text = html_escape::encode_text(&seg.normalized_text);

                let words_html = if seg.words.is_empty() {
                    escaped_text.to_string()
                } else {
                    let mut w_html = String::new();
                    for w in &seg.words {
                        let w_start_sec = w.start_ms as f64 / 1000.0;
                        let w_end_sec = w.end_ms as f64 / 1000.0;
                        let w_text = html_escape::encode_text(&w.text);
                        let _ = write!(
                            w_html,
                            r#"<span class="transcript-word" data-wstart="{w_start_sec}" data-wend="{w_end_sec}" onclick="seekWord(event, {w_start_sec})">{w_text}</span> "#
                        );
                    }
                    w_html
                };

                let _ = write!(
                    transcript_html,
                    r#"
                    <div id="turn-{idx}" class="transcript-turn" data-start="{start_sec}" data-end="{end_sec}" onclick="seekAudio({start_sec})">
                      <div class="turn-header">
                        <span class="{speaker_class}" onclick="renameSpeaker(event, '{speaker_name}')" style="cursor:pointer;" title="Click to rename this speaker">{speaker_name} ✏️ <span class="badge badge-he" style="font-size:0.65rem; margin-left:0.25rem;">{lang_code}</span></span>
                        <span class="turn-time">▶ {time_str}</span>
                      </div>
                      <div class="turn-text {dir_class}">{words_html}</div>
                    </div>
                    "#
                );
            }
        }
    } else if is_failed {
        transcript_html.push_str("<p style='color: #fca5a5; padding: 2rem; text-align:center;'>Transcription failed. See error details above.</p>");
    } else {
        transcript_html.push_str("<p style='color: var(--text-muted); padding: 2rem; text-align:center;'>⏳ Transcript processing in progress...</p>");
    }

    let star_icon = if call.is_favorite { "★" } else { "☆" };
    let star_color = if call.is_favorite {
        "#fbbf24"
    } else {
        "var(--text-muted)"
    };

    let mut tags_html = String::new();
    for tag in &call.tags {
        let escaped_tag = html_escape::encode_text(tag);
        let _ = write!(
            tags_html,
            r#"<span style="background:rgba(99,102,241,0.15); border:1px solid rgba(99,102,241,0.3); color:#a5b4fc; padding:0.2rem 0.5rem; border-radius:0.25rem; font-size:0.75rem; font-weight:600;">{escaped_tag}</span>"#
        );
    }

    let escaped_title = html_escape::encode_text(&title);

    let body = format!(
        r#"
        <div style="margin-bottom: 1.5rem; display:flex; justify-content:space-between; align-items:flex-start; flex-wrap:wrap; gap:1rem;">
          <div>
            <a href="/calls" style="color: var(--text-secondary); font-size: 0.85rem; text-decoration: none;">&larr; Back to Calls</a>
            <div style="display:flex; align-items:center; gap:0.5rem; margin-top:0.25rem;">
              <span id="detail-star" onclick="toggleDetailFavorite()" style="cursor:pointer; color:{star_color}; font-size:1.4rem; line-height:1;" title="Toggle Favorite">{star_icon}</span>
              <h1 id="call-title-display" style="font-size: 1.6rem; font-weight: 700;">{escaped_title}</h1>
              <button onclick="editTitle()" style="background:none; border:none; color:var(--text-secondary); font-size:1.1rem; cursor:pointer;" title="Edit title">✏️</button>
            </div>
            <div style="display:flex; align-items:center; gap:0.5rem; margin-top:0.35rem; flex-wrap:wrap;">
              <p style="color: var(--text-muted); font-size: 0.85rem; font-family: monospace;">UUID: {call_id}</p>
              <div id="tags-container" style="display:inline-flex; align-items:center; gap:0.35rem;">
                {tags_html}
                <button onclick="addTagPrompt()" style="background:none; border:1px dashed var(--border-color); color:var(--text-secondary); padding:0.1rem 0.4rem; border-radius:0.25rem; font-size:0.75rem; cursor:pointer;" title="Add Tag">+ Tag</button>
              </div>
            </div>
          </div>

          <!-- Exporters & Actions row -->
          <div style="display:flex; align-items:center; flex-wrap:wrap; gap:0.4rem;">
            <div style="display:inline-flex; align-items:center; gap:0.3rem;">
              <select id="reprocess-lang-select" style="font-size:0.8rem; background:var(--bg-card); border:1px solid var(--border-color); color:white; padding:0.35rem 0.5rem; border-radius:0.25rem; cursor:pointer;" title="Choose language for transcription">
                <option value="auto">🌐 Auto-Detect</option>
                <option value="he">🇮🇱 Hebrew (ivrit-ai)</option>
                <option value="ru">🇷🇺 Russian (Русский)</option>
                <option value="en">🇬🇧 English</option>
              </select>
              <button id="reprocess-btn" onclick="reprocessCall()" style="font-size:0.8rem; background:rgba(245,158,11,0.15); border:1px solid rgba(245,158,11,0.3); color:#fbbf24; padding:0.35rem 0.65rem; border-radius:0.25rem; cursor:pointer; font-weight:600; display:inline-flex; align-items:center; gap:0.25rem;" title="Re-run AI transcription & analysis">
                <span>🔄 Re-analyze</span>
              </button>
            </div>
            <button onclick="deleteCurrentCall()" style="font-size:0.8rem; background:rgba(239,68,68,0.15); border:1px solid rgba(239,68,68,0.3); color:#f87171; padding:0.35rem 0.65rem; border-radius:0.25rem; cursor:pointer; font-weight:600; display:inline-flex; align-items:center; gap:0.25rem;" title="Delete this conversation">
              <span>🗑️ Delete</span>
            </button>
            <span style="font-size:0.8rem; color:var(--text-secondary); margin-left:0.5rem; margin-right:0.25rem;">Export:</span>
            <a href="/api/v1/calls/{call_id}/export?format=srt" download style="font-size:0.8rem; background:var(--bg-card); border:1px solid var(--border-color); padding:0.35rem 0.65rem; border-radius:0.25rem; color:var(--text-secondary);">SRT</a>
            <a href="/api/v1/calls/{call_id}/export?format=vtt" download style="font-size:0.8rem; background:var(--bg-card); border:1px solid var(--border-color); padding:0.35rem 0.65rem; border-radius:0.25rem; color:var(--text-secondary);">VTT</a>
            <a href="/api/v1/calls/{call_id}/export?format=txt" download style="font-size:0.8rem; background:var(--bg-card); border:1px solid var(--border-color); padding:0.35rem 0.65rem; border-radius:0.25rem; color:var(--text-secondary);">TXT</a>
            <a href="/api/v1/calls/{call_id}/export?format=md" download style="font-size:0.8rem; background:var(--bg-card); border:1px solid var(--border-color); padding:0.35rem 0.65rem; border-radius:0.25rem; color:var(--text-secondary);">Markdown</a>
            <a href="/api/v1/calls/{call_id}/export?format=json" download style="font-size:0.8rem; background:var(--bg-card); border:1px solid var(--border-color); padding:0.35rem 0.65rem; border-radius:0.25rem; color:var(--text-secondary);">JSON</a>
            <a href="/api/v1/calls/{call_id}/export?format=ics" download style="font-size:0.8rem; background:rgba(16,185,129,0.15); border:1px solid rgba(16,185,129,0.3); padding:0.35rem 0.65rem; border-radius:0.25rem; color:#6ee7b7; font-weight:600;" title="Download Calendar appointment">📅 .ICS</a>
          </div>
        </div>

        {error_banner}

        <!-- Enhanced Audio Player -->
        <div class="audio-player-card" style="flex-direction:column; align-items:stretch; gap:0.75rem;">
          <div style="display:flex; justify-content:space-between; align-items:center; flex-wrap:wrap; gap:0.5rem;">
            <div style="display:flex; align-items:center; gap:0.5rem;">
              <button id="skip-back-btn" onclick="skipAudio(-5)" style="background:rgba(255,255,255,0.08); border:1px solid var(--border-color); color:white; padding:0.4rem 0.6rem; border-radius:0.25rem; cursor:pointer; font-size:0.85rem;" title="Rewind 5s (←)">↺ -5s</button>
              <button id="skip-fwd-btn" onclick="skipAudio(5)" style="background:rgba(255,255,255,0.08); border:1px solid var(--border-color); color:white; padding:0.4rem 0.6rem; border-radius:0.25rem; cursor:pointer; font-size:0.85rem;" title="Forward 5s (→)">↻ +5s</button>
              <div style="display:flex; align-items:center; gap:0.25rem; margin-left:0.5rem;">
                <span style="font-size:0.8rem; color:var(--text-secondary);">Speed:</span>
                <button onclick="setSpeed(1.0)" class="speed-btn active" data-speed="1" style="background:rgba(255,255,255,0.08); border:1px solid var(--border-color); color:white; padding:0.2rem 0.5rem; border-radius:0.25rem; cursor:pointer; font-size:0.8rem;">1x</button>
                <button onclick="setSpeed(1.25)" class="speed-btn" data-speed="1.25" style="background:rgba(255,255,255,0.08); border:1px solid var(--border-color); color:white; padding:0.2rem 0.5rem; border-radius:0.25rem; cursor:pointer; font-size:0.8rem;">1.25x</button>
                <button onclick="setSpeed(1.5)" class="speed-btn" data-speed="1.5" style="background:rgba(255,255,255,0.08); border:1px solid var(--border-color); color:white; padding:0.2rem 0.5rem; border-radius:0.25rem; cursor:pointer; font-size:0.8rem;">1.5x</button>
                <button onclick="setSpeed(2.0)" class="speed-btn" data-speed="2" style="background:rgba(255,255,255,0.08); border:1px solid var(--border-color); color:white; padding:0.2rem 0.5rem; border-radius:0.25rem; cursor:pointer; font-size:0.8rem;">2x</button>
              </div>
            </div>
            <div style="display:flex; align-items:center; gap:0.5rem;">
              <span style="font-size:0.75rem; color:var(--text-muted);">Hotkeys: Space (Play/Pause), ←/→ (±5s), [/] (Speed), M (Mute)</span>
              <a href="/api/v1/calls/{call_id}/recording?format=wav" download style="display:inline-flex; align-items:center; gap:0.3rem; font-size:0.85rem; color:var(--accent-blue); background:rgba(59,130,246,0.1); border:1px solid rgba(59,130,246,0.3); padding:0.35rem 0.75rem; border-radius:0.25rem;">
                <span>⬇ Download Audio (.wav)</span>
              </a>
            </div>
          </div>
          <audio id="call-audio" controls preload="auto" style="width: 100%; border-radius: 0.25rem;">
            <source src="/api/v1/calls/{call_id}/recording?format=wav" type="audio/wav">
            <source src="/api/v1/calls/{call_id}/recording" type="audio/mp4">
            <source src="/api/v1/calls/{call_id}/recording">
            Your browser does not support the audio element.
          </audio>
          <div id="audio-error-msg" style="display:none; background:rgba(239,68,68,0.15); border:1px solid var(--accent-red); color:#fca5a5; padding:0.5rem 0.75rem; border-radius:0.25rem; font-size:0.85rem;">
            Audio playback error. <a href="/api/v1/calls/{call_id}/recording" target="_blank" style="text-decoration:underline; font-weight:600;">Click here to open directly</a>.
          </div>
        </div>

        <div class="call-detail-grid">
          <!-- Left Summary Card -->
          <div class="card">
            {summary_html}
          </div>

          <!-- Right Transcript Column -->
          <div class="transcript-container">
            {transcript_html}
          </div>
        </div>

        <script>
          const audio = document.getElementById('call-audio');
          const errorMsg = document.getElementById('audio-error-msg');
          const callId = '{call_id}';

          // Global Hotkeys
          document.addEventListener('keydown', function(e) {{
            if (['INPUT', 'TEXTAREA'].includes(document.activeElement.tagName)) return;
            if (!audio) return;

            if (e.code === 'Space') {{
              e.preventDefault();
              if (audio.paused) audio.play(); else audio.pause();
            }} else if (e.code === 'ArrowLeft') {{
              e.preventDefault();
              skipAudio(-5);
            }} else if (e.code === 'ArrowRight') {{
              e.preventDefault();
              skipAudio(5);
            }} else if (e.key === '[') {{
              e.preventDefault();
              setSpeed(Math.max(0.5, audio.playbackRate - 0.25));
            }} else if (e.key === ']') {{
              e.preventDefault();
              setSpeed(Math.min(2.5, audio.playbackRate + 0.25));
            }} else if (e.key.toLowerCase() === 'm') {{
              e.preventDefault();
              audio.muted = !audio.muted;
            }}
          }});

          if (audio) {{
            audio.addEventListener('error', function(e) {{
              console.error('Audio load error:', e);
              if (errorMsg) errorMsg.style.display = 'block';
            }});

            audio.addEventListener('timeupdate', function() {{
              const currentTime = audio.currentTime;
              const turns = document.querySelectorAll('.transcript-turn');
              turns.forEach(turn => {{
                const start = parseFloat(turn.getAttribute('data-start') || '0');
                const end = parseFloat(turn.getAttribute('data-end') || '0');
                if (currentTime >= start && currentTime <= end) {{
                  turn.classList.add('active-turn');
                }} else {{
                  turn.classList.remove('active-turn');
                }}
              }});

              const words = document.querySelectorAll('.transcript-word');
              words.forEach(w => {{
                const wstart = parseFloat(w.getAttribute('data-wstart') || '0');
                const wend = parseFloat(w.getAttribute('data-wend') || '0');
                if (currentTime >= wstart && currentTime <= wend) {{
                  w.classList.add('active-word');
                }} else {{
                  w.classList.remove('active-word');
                }}
              }});
            }});
          }}

          function seekWord(event, seconds) {{
            event.stopPropagation();
            seekAudio(seconds);
          }}

          function seekAudio(seconds) {{
            if (audio) {{
              audio.currentTime = seconds;
              audio.play().catch(e => console.log('Autoplay prevented:', e));
            }}
          }}

          function skipAudio(delta) {{
            if (audio) {{
              audio.currentTime = Math.max(0, audio.currentTime + delta);
            }}
          }}

          function setSpeed(rate) {{
            if (audio) {{
              audio.playbackRate = rate;
              document.querySelectorAll('.speed-btn').forEach(b => {{
                if (parseFloat(b.getAttribute('data-speed')) === rate) {{
                  b.style.background = 'var(--accent-blue)';
                  b.style.borderColor = 'var(--accent-blue)';
                }} else {{
                  b.style.background = 'rgba(255,255,255,0.08)';
                  b.style.borderColor = 'var(--border-color)';
                }}
              }});
            }}
          }}

          async function toggleDetailFavorite() {{
            const star = document.getElementById('detail-star');
            try {{
              const res = await fetch(`/api/v1/calls/${{callId}}/favorite`, {{ method: 'POST' }});
              if (res.ok) {{
                const data = await res.json();
                if (star) {{
                  star.innerText = data.is_favorite ? '★' : '☆';
                  star.style.color = data.is_favorite ? '#fbbf24' : 'var(--text-muted)';
                }}
              }}
            }} catch (e) {{}}
          }}

          async function addTagPrompt() {{
            const tag = prompt('Enter tag name (e.g. Family, Work, Doctor):');
            if (tag && tag.trim()) {{
              try {{
                const tags = Array.from(document.querySelectorAll('#tags-container span')).map(s => s.innerText);
                tags.push(tag.trim());
                const res = await fetch(`/api/v1/calls/${{callId}}/tags`, {{
                  method: 'PUT',
                  headers: {{ 'Content-Type': 'application/json' }},
                  body: JSON.stringify({{ tags }})
                }});
                if (res.ok) window.location.reload();
              }} catch (e) {{}}
            }}
          }}

          async function reprocessCall() {{
            const langSelect = document.getElementById('reprocess-lang-select');
            const lang = langSelect ? langSelect.value : 'auto';
            if (!confirm(`Re-run transcription & AI analysis with selected language mode (${{lang}})?`)) return;
            const btn = document.getElementById('reprocess-btn');
            btn.disabled = true;
            btn.innerText = '⏳ Queuing...';
            try {{
              const res = await fetch(`/api/v1/calls/${{callId}}/reprocess?language=${{lang}}`, {{ method: 'POST' }});
              if (res.ok) {{
                btn.innerText = '✓ Queued!';
                setTimeout(() => window.location.reload(), 2500);
              }} else {{
                alert('Failed to queue reprocess.');
                btn.disabled = false;
                btn.innerText = '🔄 Re-analyze';
              }}
            }} catch (err) {{
              alert(`Error: ${{err.message}}`);
              btn.disabled = false;
              btn.innerText = '🔄 Re-analyze';
            }}
          }}

          async function deleteCurrentCall() {{
            if (!confirm('Are you sure you want to permanently delete this conversation and its recording?')) return;
            try {{
              const res = await fetch(`/api/v1/calls/${{callId}}`, {{ method: 'DELETE' }});
              if (res.ok || res.status === 204) {{
                window.location.href = '/calls';
              }} else {{
                alert('Failed to delete conversation.');
              }}
            }} catch (err) {{
              alert(`Error: ${{err.message}}`);
            }}
          }}

          async function editTitle() {{
            const currentTitle = document.getElementById('call-title-display').innerText;
            const newTitle = prompt('Enter new conversation title:', currentTitle);
            if (newTitle && newTitle.trim() && newTitle !== currentTitle) {{
              try {{
                const res = await fetch(`/api/v1/calls/${{callId}}`, {{
                  method: 'PATCH',
                  headers: {{ 'Content-Type': 'application/json' }},
                  body: JSON.stringify({{ title: newTitle.trim() }})
                }});
                if (res.ok) {{
                  document.getElementById('call-title-display').innerText = newTitle.trim();
                }} else {{
                  alert('Failed to update title.');
                }}
              }} catch (err) {{
                alert(`Error: ${{err.message}}`);
              }}
            }}
          }}

          function renameSpeaker(event, oldName) {{
            event.stopPropagation();
            const newName = prompt(`Rename "${{oldName}}" to:`, oldName);
            if (newName && newName.trim() && newName !== oldName) {{
              document.querySelectorAll('.turn-header span').forEach(el => {{
                if (el.innerText.startsWith(oldName)) {{
                  el.childNodes[0].nodeValue = `${{newName.trim()}} `;
                }}
              }});
            }}
          }}
          function showToast(msg) {{
            const t = document.createElement('div');
            t.innerText = msg;
            t.style.cssText = 'position:fixed; bottom:24px; right:24px; background:#10b981; color:white; padding:10px 18px; border-radius:6px; font-weight:600; z-index:9999; box-shadow:0 4px 12px rgba(0,0,0,0.3);';
            document.body.appendChild(t);
            setTimeout(() => t.remove(), 2500);
          }}
        </script>
        "#
    );

    render_layout(&title, "calls", &body)
}

fn urlencoding_simple(s: &str) -> String {
    let mut encoded = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            b' ' => encoded.push_str("%20"),
            b'\n' => encoded.push_str("%0A"),
            _ => {
                let _ = write!(encoded, "%{:02X}", b);
            }
        }
    }
    encoded
}
