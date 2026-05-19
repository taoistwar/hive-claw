//! Minimal prompt-template renderer (port of
//! `nanobot.utils.prompt_templates`).
//!
//! The Python original uses Jinja2 against `templates/` under the installed
//! package. There is no drop-in equivalent in Rust that is as widely used as
//! Jinja2 without pulling a heavy dependency (e.g. `minijinja`, `tera`).
//! For now we offer:
//!
//! * A [`TemplateLoader`] trait that any consumer can implement.
//! * A simple `{{ key }}` substitution engine so the most common
//!   template shape works out-of-the-box.
//! * A feature-gated place for callers to upgrade to a real engine later.
//!
//! Callers that need blocks / inheritance should swap in a `minijinja`-backed
//! implementation of [`TemplateLoader`] in their own crate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;

/// Source of a template (typically a file on disk).
pub trait TemplateLoader: Send + Sync {
    fn load(&self, name: &str) -> Option<String>;
}

/// A [`TemplateLoader`] that reads files relative to a root directory
/// (mirrors Python's `FileSystemLoader`).
#[derive(Debug, Clone)]
pub struct FileSystemLoader {
    root: PathBuf,
}

impl FileSystemLoader {
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }
}

impl TemplateLoader for FileSystemLoader {
    fn load(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(name)).ok()
    }
}

static VAR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{\{\s*(\w+)\s*\}\}").unwrap());

/// Render a template by performing `{{ key }}` substitution. Unknown
/// keys are left intact so template authors can see missing context.
///
/// Optionally strip trailing whitespace for single-line values.
pub fn render_template(
    loader: &dyn TemplateLoader,
    name: &str,
    context: &HashMap<String, String>,
    strip: bool,
) -> Option<String> {
    let src = loader.load(name)?;
    let out = VAR_RE.replace_all(&src, |caps: &regex::Captures| match context.get(&caps[1]) {
        Some(v) => v.clone(),
        None => caps[0].to_string(),
    });
    Some(if strip {
        out.trim_end().to_string()
    } else {
        out.into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InMemory(HashMap<String, String>);
    impl TemplateLoader for InMemory {
        fn load(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    #[test]
    fn substitutes_known_keys() {
        let templates = HashMap::from([
            ("hi.md".to_string(), "Hello, {{ name }}!\n".to_string()),
        ]);
        let loader = InMemory(templates);
        let ctx = HashMap::from([("name".to_string(), "world".to_string())]);
        assert_eq!(
            render_template(&loader, "hi.md", &ctx, true).unwrap(),
            "Hello, world!"
        );
    }
}
