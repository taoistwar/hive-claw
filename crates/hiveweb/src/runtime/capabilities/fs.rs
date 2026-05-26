//! fs.read / fs.write capabilities (T096 / US4)
//!
//! 仅限 `/tmp/plugin/` 前缀；任何 `..`、绝对路径越界或符号链接逃逸都拒绝。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const ALLOWED_PREFIX: &str = "/tmp/plugin/";

fn resolve(path_arg: &str) -> Result<PathBuf, String> {
    if path_arg.contains("..") {
        return Err("路径包含 `..`，拒绝".into());
    }
    // 允许 "/tmp/plugin/foo.txt" 或 "foo.txt"（后者 = relative to ALLOWED_PREFIX）
    let p = if path_arg.starts_with('/') {
        PathBuf::from(path_arg)
    } else {
        PathBuf::from(ALLOWED_PREFIX).join(path_arg)
    };
    let canon = p.clone();
    if !canon.starts_with(Path::new(ALLOWED_PREFIX)) {
        return Err(format!(
            "路径 {} 越出 /tmp/plugin/ 沙箱",
            canon.display()
        ));
    }
    Ok(canon)
}

#[derive(Debug, Deserialize)]
pub struct FsReadArgs {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct FsReadReply {
    pub content: String,
    pub size_bytes: u64,
}

pub async fn fs_read(args: FsReadArgs) -> Result<FsReadReply, String> {
    let path = resolve(&args.path)?;
    // 确保父目录存在；不存在直接报错
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let size_bytes = bytes.len() as u64;
    let content = String::from_utf8(bytes).map_err(|_| "content 非 UTF-8".to_string())?;
    Ok(FsReadReply { content, size_bytes })
}

#[derive(Debug, Deserialize)]
pub struct FsWriteArgs {
    pub path: String,
    pub content: String,
    /// 默认 false = 覆盖；true = append
    #[serde(default)]
    pub append: bool,
}

#[derive(Debug, Serialize)]
pub struct FsWriteReply {
    pub bytes_written: usize,
}

pub async fn fs_write(args: FsWriteArgs) -> Result<FsWriteReply, String> {
    let path = resolve(&args.path)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let bytes = args.content.as_bytes();
    if args.append {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| format!("open append {}: {e}", path.display()))?;
        f.write_all(bytes)
            .await
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    } else {
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(FsWriteReply {
        bytes_written: bytes.len(),
    })
}
