/// Modern dark-theme responsive CSS stylesheet for CallMind.
pub const APP_CSS: &str = r#"
:root {
  --bg-primary: #0f172a;
  --bg-secondary: #1e293b;
  --bg-card: #1e293b;
  --bg-card-hover: #334155;
  --text-primary: #f8fafc;
  --text-secondary: #94a3b8;
  --text-muted: #64748b;
  --border-color: #334155;
  --accent-blue: #3b82f6;
  --accent-indigo: #6366f1;
  --accent-green: #10b981;
  --accent-amber: #f59e0b;
  --accent-red: #ef4444;
}

* {
  box-sizing: border-box;
  margin: 0;
  padding: 0;
}

body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  background-color: var(--bg-primary);
  color: var(--text-primary);
  line-height: 1.5;
  -webkit-font-smoothing: antialiased;
}

a {
  color: inherit;
  text-decoration: none;
}

/* Layout */
.app-container {
  display: flex;
  flex-direction: column;
  min-height: 100vh;
}

.navbar {
  background-color: var(--bg-secondary);
  border-bottom: 1px solid var(--border-color);
  padding: 0.75rem 1.5rem;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.nav-brand {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 1.25rem;
  font-weight: 700;
  color: #60a5fa;
}

.nav-links {
  display: flex;
  gap: 1.5rem;
  font-size: 0.95rem;
  font-weight: 500;
}

.nav-link {
  color: var(--text-secondary);
  transition: color 0.15s ease;
}

.nav-link:hover, .nav-link.active {
  color: var(--text-primary);
}

.main-content {
  flex: 1;
  padding: 1.5rem;
  max-width: 1400px;
  margin: 0 auto;
  width: 100%;
}

/* Cards & Grid */
.grid-4 {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  gap: 1rem;
  margin-bottom: 1.5rem;
}

.card {
  background-color: var(--bg-card);
  border: 1px solid var(--border-color);
  border-radius: 0.5rem;
  padding: 1.25rem;
}

.card-title {
  font-size: 0.85rem;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin-bottom: 0.5rem;
}

.card-value {
  font-size: 1.75rem;
  font-weight: 700;
}

/* Call Split View */
.call-detail-grid {
  display: grid;
  grid-template-columns: 450px 1fr;
  gap: 1.5rem;
  align-items: start;
}

@media (max-width: 1024px) {
  .call-detail-grid {
    grid-template-columns: 1fr;
  }
}

/* Audio Player */
.audio-player-card {
  background-color: var(--bg-card);
  border: 1px solid var(--border-color);
  border-radius: 0.5rem;
  padding: 1rem;
  margin-bottom: 1.5rem;
  display: flex;
  align-items: center;
  gap: 1rem;
}

.audio-controls {
  display: flex;
  align-items: center;
  gap: 0.75rem;
}

.play-btn {
  background-color: var(--accent-blue);
  color: white;
  border: none;
  border-radius: 50%;
  width: 40px;
  height: 40px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 1.2rem;
}

.audio-slider {
  flex: 1;
  accent-color: var(--accent-blue);
  cursor: pointer;
}

/* Transcript Bubbles */
.transcript-container {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.transcript-turn {
  background-color: var(--bg-card);
  border: 1px solid var(--border-color);
  border-radius: 0.5rem;
  padding: 1rem;
  transition: background-color 0.15s ease, border-color 0.15s ease;
  cursor: pointer;
}

.transcript-turn:hover {
  background-color: var(--bg-card-hover);
  border-color: var(--accent-blue);
}

.transcript-turn.active-turn {
  border-color: #3b82f6;
  background-color: rgba(59, 130, 246, 0.15);
  box-shadow: 0 0 12px rgba(59, 130, 246, 0.25);
}

.turn-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 0.5rem;
  font-size: 0.85rem;
}

.turn-speaker-1, .turn-speaker-agent {
  color: #60a5fa;
  font-weight: 600;
}

.turn-speaker-2, .turn-speaker-customer {
  color: #34d399;
  font-weight: 600;
}

.turn-speaker-3, .turn-speaker-supervisor {
  color: #fbbf24;
  font-weight: 600;
}

.turn-speaker-4, .turn-speaker-participant {
  color: #c084fc;
  font-weight: 600;
}

.turn-time {
  color: var(--text-muted);
  font-family: monospace;
}

.turn-text {
  font-size: 1.05rem;
  line-height: 1.6;
}

.transcript-word {
  display: inline-block;
  padding: 0.05rem 0.15rem;
  border-radius: 0.2rem;
  transition: background-color 0.1s ease, color 0.1s ease;
  cursor: pointer;
}

.transcript-word:hover {
  background-color: rgba(59, 130, 246, 0.3);
  color: #93c5fd;
}

.transcript-word.active-word {
  background-color: #3b82f6;
  color: #ffffff;
  font-weight: 600;
  border-radius: 0.25rem;
  box-shadow: 0 0 8px rgba(59, 130, 246, 0.5);
}

/* Directionality */
.dir-rtl {
  direction: rtl;
  text-align: right;
}

.dir-ltr {
  direction: ltr;
  text-align: left;
}

/* Badges */
.badge {
  display: inline-block;
  padding: 0.2rem 0.5rem;
  border-radius: 0.25rem;
  font-size: 0.75rem;
  font-weight: 600;
  text-transform: uppercase;
}

.badge-he { background-color: #1e3a8a; color: #93c5fd; }
.badge-ru { background-color: #3b0764; color: #d8b4fe; }
.badge-en { background-color: #064e3b; color: #6ee7b7; }
.badge-mix { background-color: #78350f; color: #fde68a; }

.badge-completed { background-color: rgba(16, 185, 129, 0.2); color: #34d399; }
.badge-pending { background-color: rgba(245, 158, 11, 0.2); color: #fbbf24; }

/* Table */
.table-container {
  overflow-x: auto;
  border: 1px solid var(--border-color);
  border-radius: 0.5rem;
  background-color: var(--bg-card);
}

table {
  width: 100%;
  border-collapse: collapse;
  text-align: left;
  font-size: 0.9rem;
}

th, td {
  padding: 0.75rem 1rem;
  border-bottom: 1px solid var(--border-color);
}

th {
  background-color: rgba(0,0,0,0.2);
  color: var(--text-secondary);
  font-weight: 600;
  text-transform: uppercase;
  font-size: 0.75rem;
  letter-spacing: 0.05em;
}

tr:hover td {
  background-color: var(--bg-card-hover);
}
"#;
