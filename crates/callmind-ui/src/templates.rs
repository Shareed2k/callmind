//! Runtime HTML templating, and the extension point plugins render through.
//!
//! Templates are loaded at runtime rather than compiled in, because a plugin has
//! to be able to contribute its own view without rebuilding the core. That rules
//! out `askama`-style compile-time engines.
//!
//! Auto-escaping is forced on for every template. The views this replaces built
//! HTML by string interpolation, which is how a stored XSS got into the to-do
//! list: escaping by default makes that class of bug structural rather than a
//! thing each author has to remember.

use minijinja::{AutoEscape, Environment, Value};
use std::collections::BTreeMap;

/// Built-in views, compiled into the binary but rendered at runtime.
const BUILTIN: &[(&str, &str)] = &[
    ("emotions", include_str!("../templates/emotions.html")),
    (
        "plugin/acoustic-emotions",
        include_str!("../templates/speaker_emotions.html"),
    ),
];

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("template {name:?} failed: {source}")]
    Render {
        name: String,
        #[source]
        source: minijinja::Error,
    },
    #[error("plugin name {0:?} is not a valid identifier")]
    InvalidPluginName(String),
}

/// Holds the template environment, including anything plugins have registered.
pub struct TemplateRegistry {
    env: Environment<'static>,
}

impl Default for TemplateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateRegistry {
    #[must_use]
    pub fn new() -> Self {
        let mut env = Environment::new();
        // Always escape, regardless of the template's name. minijinja's default
        // decides from the file extension, which would silently stop escaping a
        // plugin template registered under a name without one.
        env.set_auto_escape_callback(|_name| AutoEscape::Html);

        for (name, source) in BUILTIN {
            // Built-in sources are compiled in, so a failure here is a bug in
            // this crate rather than bad input.
            env.add_template(name, source)
                .expect("built-in template should compile");
        }

        Self { env }
    }

    /// Register a view supplied by a plugin.
    ///
    /// The template is trusted code — installing a plugin is already a trust
    /// decision — but the *data* it renders is not, which is what auto-escaping
    /// covers.
    pub fn register_plugin_template(
        &mut self,
        plugin: &str,
        source: String,
    ) -> Result<(), TemplateError> {
        if plugin.is_empty()
            || !plugin
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(TemplateError::InvalidPluginName(plugin.to_string()));
        }
        let name = format!("plugin/{plugin}");
        self.env
            .add_template_owned(name.clone(), source)
            .map_err(|source| TemplateError::Render { name, source })
    }

    /// True when something can render this plugin's results.
    #[must_use]
    pub fn has_plugin_view(&self, plugin: &str) -> bool {
        self.env.get_template(&format!("plugin/{plugin}")).is_ok()
    }

    /// Render a plugin's stored payload.
    ///
    /// Returns `None` when no view is registered, so an unknown plugin's results
    /// are simply not shown rather than breaking the page.
    pub fn render_plugin(
        &self,
        plugin: &str,
        payload: &serde_json::Value,
    ) -> Option<Result<String, TemplateError>> {
        let name = format!("plugin/{plugin}");
        let template = self.env.get_template(&name).ok()?;
        Some(
            template
                .render(Value::from_serialize(payload))
                .map_err(|source| TemplateError::Render { name, source }),
        )
    }

    /// Render a built-in view.
    pub fn render<S: serde::Serialize>(
        &self,
        name: &str,
        context: &S,
    ) -> Result<String, TemplateError> {
        self.env
            .get_template(name)
            .and_then(|t| t.render(Value::from_serialize(context)))
            .map_err(|source| TemplateError::Render {
                name: name.to_string(),
                source,
            })
    }

    /// Render every stored plugin result that has a view, in a stable order.
    #[must_use]
    pub fn render_all_plugins(&self, results: &[(String, String)]) -> String {
        let mut sections = BTreeMap::new();
        for (plugin, payload_json) in results {
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(payload_json) else {
                tracing::warn!("Plugin {plugin} stored a payload that is not JSON; skipping");
                continue;
            };
            match self.render_plugin(plugin, &payload) {
                Some(Ok(html)) => {
                    sections.insert(plugin.clone(), html);
                }
                Some(Err(e)) => tracing::warn!("Plugin {plugin} view failed to render: {e}"),
                // No view registered for this plugin.
                None => {}
            }
        }
        sections.into_values().collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The reason for moving to a template engine at all. The previous views
    /// interpolated strings by hand, which is how a stored XSS reached the to-do
    /// list. Escaping must hold for built-in and plugin templates alike.
    #[test]
    fn data_is_escaped_in_builtin_views() {
        let registry = TemplateRegistry::new();
        let html = registry
            .render(
                "emotions",
                &json!({
                    "dominant": "<script>alert(1)</script>",
                    "scores": [{ "label": "\"><img src=x onerror=alert(1)>", "percent": 42 }]
                }),
            )
            .unwrap();

        // The property that matters is that no `<` or `"` from data reaches the
        // output unescaped. The literal text "onerror=alert(1)" may well appear
        // — as inert text content, which is exactly the point.
        assert!(!html.contains("<script"), "script tag survived: {html}");
        assert!(!html.contains("<img"), "img tag survived: {html}");
        // Assert on the payload specifically: checking for `">` anywhere would
        // match the template's own static markup.
        assert!(html.contains("&lt;script&gt;"));
        assert!(
            html.contains("&quot;&gt;&lt;img src=x onerror=alert(1)&gt;"),
            "payload was not escaped as a whole: {html}"
        );
    }

    #[test]
    fn data_is_escaped_in_plugin_views() {
        let mut registry = TemplateRegistry::new();
        // A plugin template is trusted code; the data it renders is not.
        registry
            .register_plugin_template("demo", "<p>{{ note }}</p>".to_string())
            .unwrap();

        let html = registry
            .render_plugin("demo", &json!({ "note": "<script>alert(1)</script>" }))
            .unwrap()
            .unwrap();
        // minijinja also escapes `/`, hence `&#x2f;`.
        assert!(!html.contains("<script"), "{html}");
        assert!(
            html.contains("&lt;script&gt;alert(1)&lt;&#x2f;script&gt;"),
            "{html}"
        );
    }

    /// Escaping must not depend on the template name having an extension —
    /// minijinja's default callback decides from one, which would silently stop
    /// escaping a plugin view registered as "plugin/foo".
    #[test]
    fn escaping_does_not_depend_on_a_file_extension() {
        let mut registry = TemplateRegistry::new();
        registry
            .register_plugin_template("no_extension_here", "{{ x }}".to_string())
            .unwrap();
        let html = registry
            .render_plugin("no_extension_here", &json!({ "x": "<b>" }))
            .unwrap()
            .unwrap();
        assert_eq!(html, "&lt;b&gt;");
    }

    #[test]
    fn plugin_names_are_restricted() {
        let mut registry = TemplateRegistry::new();
        // A name is used as a storage key and a template path.
        for bad in ["", "../escape", "with space", "semi;colon"] {
            assert!(
                registry
                    .register_plugin_template(bad, "{{ x }}".into())
                    .is_err(),
                "accepted {bad:?}"
            );
        }
        assert!(
            registry
                .register_plugin_template("acoustic-emotions_v2", "{{ x }}".into())
                .is_ok()
        );
    }

    #[test]
    fn unknown_plugins_are_skipped_not_fatal() {
        let registry = TemplateRegistry::new();
        assert!(
            registry
                .render_plugin("never-registered", &json!({}))
                .is_none()
        );

        // A page must still render when one plugin's payload is unusable.
        let rendered = registry.render_all_plugins(&[
            ("never-registered".into(), "{}".into()),
            ("acoustic-emotions".into(), "not json at all".into()),
        ]);
        assert!(rendered.is_empty());
    }

    #[test]
    fn acoustic_emotion_results_render() {
        let registry = TemplateRegistry::new();
        let payload = json!({
            "kind": "speaker_emotions",
            "model": "wav2vec2-emotion",
            "summaries": [{
                "speaker_id": 1,
                "dominant": "joy",
                "scores": [{ "emotion": "joy", "score": 0.72 }, { "emotion": "anger", "score": 0.05 }]
            }],
            "spans": []
        });
        let html = registry
            .render_plugin("acoustic-emotions", &payload)
            .unwrap()
            .unwrap();
        assert!(html.contains("Speaker 1"));
        assert!(html.contains("joy"));
        assert!(
            html.contains("72%"),
            "score not rendered as a whole percentage: {html}"
        );
        assert!(html.contains("wav2vec2-emotion"));
    }
}
