//! secret.get capability handler (T100 / US4)
//!
//! env-backed allowlist：SECRET_ALLOWLIST=key1,key2,... 列出允许 Plugin 读取的 env 名。
//! 危险 capability — 必须 Super role 才能给 Agent 授予。

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct SecretGetArgs {
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct SecretGetReply {
    pub value: String,
}

pub fn secret_get(args: SecretGetArgs) -> Result<SecretGetReply, String> {
    let allowlist: Vec<String> = std::env::var("SECRET_ALLOWLIST")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if !allowlist.iter().any(|k| k == &args.key) {
        return Err(format!("secret key '{}' 不在 allowlist 内", args.key));
    }
    let value =
        std::env::var(&args.key).map_err(|_| format!("secret key '{}' 未设置 env 值", args.key))?;
    Ok(SecretGetReply { value })
}
