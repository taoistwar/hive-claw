//! Bundled skills assets.
//!
//! The Python `nanobot.skills` package is pure data (markdown + a handful of
//! helper scripts). We embed the whole directory at compile time via
//! `include_dir` so downstream crates can read skill content without
//! shipping extra files at runtime.

use std::path::Path;

use include_dir::{Dir, include_dir};

/// The embedded `skills/` tree rooted at this crate's `assets/` directory.
pub static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets");

/// Names of every top-level skill directory (e.g. `cron`, `github`, ...).
pub fn skill_names() -> Vec<&'static str> {
    SKILLS
        .dirs()
        .filter_map(|d| d.path().file_name().and_then(|s| s.to_str()))
        .collect()
}

/// Return the `SKILL.md` contents for the given skill name, if present.
pub fn skill_manifest(name: &str) -> Option<&'static str> {
    let path = Path::new(name).join("SKILL.md");
    SKILLS.get_file(path)?.contents_utf8()
}

/// Look up an arbitrary embedded file inside the skills tree.
pub fn get_file(relative: &str) -> Option<&'static include_dir::File<'static>> {
    SKILLS.get_file(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_expected_skills() {
        let names = skill_names();
        for expected in ["cron", "github", "memory", "weather"] {
            assert!(names.contains(&expected), "missing skill: {expected}");
        }
    }

    #[test]
    fn manifests_readable() {
        assert!(skill_manifest("cron").is_some());
    }
}
