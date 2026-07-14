//! Static hosting for the web-admin single-page application.

use std::io;
use std::path::{Path, PathBuf};

use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

use crate::app_mode::AppMode;

/// Resolve the web-admin build directory for the current process.
pub fn dist_dir(mode: AppMode) -> io::Result<PathBuf> {
    if mode == AppMode::Development {
        return Ok(development_dist_dir());
    }

    std::env::current_exe().map(|executable| dist_dir_for(mode, &executable))
}

/// Resolve the build directory using an explicit executable path.
///
/// Development reads the workspace's `web-admin/dist`. Test and production
/// use a `dist` directory next to the `hiveweb` executable.
pub fn dist_dir_for(mode: AppMode, executable: &Path) -> PathBuf {
    match mode {
        AppMode::Development => development_dist_dir(),
        AppMode::Test | AppMode::Production => executable
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("dist"),
    }
}

/// Mount static assets and an SPA history fallback under `/web-admin`.
pub fn serve_dist(app: Router, dist: &Path) -> Router {
    let index = dist.join("index.html");
    let service = ServeDir::new(dist).fallback(ServeFile::new(index));
    app.nest_service("/web-admin", service)
}

fn development_dist_dir() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(crate_dir)
        .join("web-admin/dist")
}
