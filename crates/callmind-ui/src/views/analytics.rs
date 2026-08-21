use crate::views::layout::render_layout;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// Real statistics model for Analytics Dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsData {
    pub total_calls: u64,
    pub completed_calls: u64,
    pub avg_duration_secs: f64,
    pub total_audio_hours: f64,
    pub hebrew_percent: u32,
    pub russian_percent: u32,
    pub english_percent: u32,
    pub top_intents: Vec<(String, u64, u32)>, // (name, count, percent)
    pub daily_counts: Vec<(String, u64)>,     // (date YYYY-MM-DD, count)
}

/// Render the Analytics Dashboard HTML with real SQLite database statistics.
#[must_use]
pub fn render_analytics_dashboard(data: &AnalyticsData) -> String {
    let completion_rate = if data.total_calls > 0 {
        (data.completed_calls as f64 / data.total_calls as f64) * 100.0
    } else {
        0.0
    };

    let avg_dur_mins = (data.avg_duration_secs / 60.0).floor() as u32;
    let avg_dur_rem_secs = (data.avg_duration_secs % 60.0).round() as u32;
    let avg_duration_display = format!("{avg_dur_mins:02}:{avg_dur_rem_secs:02}");

    let mut topics_html = String::new();
    if data.top_intents.is_empty() {
        topics_html.push_str("<p style='color: var(--text-muted); font-size:0.85rem;'>No conversation topics categorized yet.</p>");
    } else {
        for (intent, count, pct) in &data.top_intents {
            let escaped_intent = html_escape::encode_text(intent);
            let _ = write!(
                topics_html,
                r#"
                <div>
                  <div style="display:flex; justify-content:space-between; font-size: 0.85rem; margin-bottom: 0.25rem;">
                    <span style="font-weight:500;">{escaped_intent}</span>
                    <span style="color:var(--text-secondary);">{count} calls ({pct}%)</span>
                  </div>
                  <div style="background: rgba(255,255,255,0.08); height: 6px; border-radius: 3px; overflow:hidden;">
                    <div style="background: linear-gradient(90deg, #3b82f6, #6366f1); width: {pct}%; height: 100%;"></div>
                  </div>
                </div>
                "#
            );
        }
    }

    // Daily activity sparkline / bars
    let mut activity_bars_html = String::new();
    let max_daily = data
        .daily_counts
        .iter()
        .map(|(_, c)| *c)
        .max()
        .unwrap_or(1)
        .max(1);
    for (day, count) in &data.daily_counts {
        let height_pct = ((*count as f64 / max_daily as f64) * 100.0).max(8.0) as u32;
        let day_label = if day.len() >= 5 { &day[5..] } else { day };
        let _ = write!(
            activity_bars_html,
            r#"
            <div style="flex:1; display:flex; flex-direction:column; align-items:center; gap:0.3rem;">
              <div style="font-size:0.7rem; color:var(--text-muted);">{count}</div>
              <div style="background:rgba(255,255,255,0.05); width:100%; height:80px; border-radius:0.25rem; display:flex; align-items:flex-end;">
                <div style="background:linear-gradient(180deg, #3b82f6, #1d4ed8); width:100%; height:{height_pct}%; border-radius:0.25rem;" title="{day}: {count} calls"></div>
              </div>
              <div style="font-size:0.65rem; color:var(--text-secondary);">{day_label}</div>
            </div>
            "#
        );
    }

    let body = format!(
        r#"
        <div style="margin-bottom: 1.5rem;">
          <h1 style="font-size: 1.75rem; font-weight: 700;">Conversation Analytics</h1>
          <p style="color: var(--text-secondary); font-size: 0.9rem;">Live aggregated intelligence from your personal conversation database.</p>
        </div>

        <div class="grid-4">
          <div class="card">
            <div class="card-title">Total Conversations</div>
            <div class="card-value">{}</div>
          </div>
          <div class="card">
            <div class="card-title">Processed & Ready</div>
            <div class="card-value" style="color: #34d399;">{:.1}% <span style="font-size:0.85rem; font-weight:normal; color:var(--text-secondary);">({}/{})</span></div>
          </div>
          <div class="card">
            <div class="card-title">Average Call Length</div>
            <div class="card-value" style="color: #60a5fa;">{avg_duration_display}</div>
          </div>
          <div class="card">
            <div class="card-title">Total Audio Transcribed</div>
            <div class="card-value" style="color: #fbbf24;">{:.1} <span style="font-size:0.85rem; font-weight:normal; color:var(--text-secondary);">hours</span></div>
          </div>
        </div>

        <!-- Daily Activity Timeline -->
        <div class="card" style="margin-bottom: 1.5rem;">
          <h3 style="font-size: 1rem; margin-bottom: 1rem;">Daily Recording Activity</h3>
          <div style="display:flex; gap:0.5rem; align-items:flex-end; overflow-x:auto; padding-bottom:0.5rem;">
            {activity_bars_html}
          </div>
        </div>

        <div style="display:grid; grid-template-columns: 1fr 1fr; gap: 1.5rem;">
          <div class="card">
            <h3 style="font-size: 1rem; margin-bottom: 1.25rem;">Language Distribution</h3>
            <div style="display:flex; flex-direction:column; gap: 1rem;">
              <div>
                <div style="display:flex; justify-content:space-between; font-size: 0.85rem; margin-bottom: 0.25rem;">
                  <span>Hebrew (עברית)</span>
                  <span>{}%</span>
                </div>
                <div style="background: rgba(255,255,255,0.08); height: 8px; border-radius: 4px; overflow:hidden;">
                  <div style="background: #3b82f6; width: {}%; height: 100%;"></div>
                </div>
              </div>

              <div>
                <div style="display:flex; justify-content:space-between; font-size: 0.85rem; margin-bottom: 0.25rem;">
                  <span>Russian (Русский)</span>
                  <span>{}%</span>
                </div>
                <div style="background: rgba(255,255,255,0.08); height: 8px; border-radius: 4px; overflow:hidden;">
                  <div style="background: #a855f7; width: {}%; height: 100%;"></div>
                </div>
              </div>

              <div>
                <div style="display:flex; justify-content:space-between; font-size: 0.85rem; margin-bottom: 0.25rem;">
                  <span>English & Other</span>
                  <span>{}%</span>
                </div>
                <div style="background: rgba(255,255,255,0.08); height: 8px; border-radius: 4px; overflow:hidden;">
                  <div style="background: #10b981; width: {}%; height: 100%;"></div>
                </div>
              </div>
            </div>
          </div>

          <div class="card">
            <h3 style="font-size: 1rem; margin-bottom: 1.25rem;">Top Discussion Topics & Intents</h3>
            <div style="display:flex; flex-direction:column; gap: 0.85rem;">
              {topics_html}
            </div>
          </div>
        </div>
        "#,
        data.total_calls,
        completion_rate,
        data.completed_calls,
        data.total_calls,
        data.total_audio_hours,
        data.hebrew_percent,
        data.hebrew_percent,
        data.russian_percent,
        data.russian_percent,
        data.english_percent,
        data.english_percent,
    );

    render_layout("Analytics", "analytics", &body)
}
