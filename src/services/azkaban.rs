//! Azkaban 服务
//! 
//! 负责与 Azkaban API 交互，管理任务的创建、执行和监控

use crate::config::AppConfig;
use anyhow::{Result, Context};
use reqwest::Client;
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Azkaban 执行状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum ExecStatus {
    Prepared,
    Running,
    Succeeded,
    Failed,
    Killed,
}

/// Azkaban 执行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Execution {
    pub exec_id: i64,
    pub project_name: String,
    pub flow_name: String,
    pub status: ExecStatus,
    pub submit_time: i64,
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
    pub submit_user: String,
}

/// Azkaban 项目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzkabanProject {
    pub name: String,
    pub description: Option<String>,
}

/// Azkaban 服务
pub struct AzkabanService {
    config: AppConfig,
    client: Arc<RwLock<Option<Client>>>,
    session_id: Arc<RwLock<Option<String>>>,
    base_url: String,
}

impl AzkabanService {
    /// 创建新的 Azkaban 服务
    pub fn new(config: AppConfig) -> Self {
        let base_url = config.azkaban.host.clone();
        Self {
            config,
            client: Arc::new(RwLock::new(None)),
            session_id: Arc::new(RwLock::new(None)),
            base_url,
        }
    }

    /// 初始化 Azkaban 客户端
    pub async fn initialize(&self) -> Result<()> {
        let client = Client::builder()
            .cookie_store(true)
            .build()
            .context("Failed to create HTTP client")?;

        *self.client.write().await = Some(client);
        tracing::info!("Azkaban client initialized for {}", self.base_url);

        // 登录获取 session
        self.login().await?;

        Ok(())
    }

    /// 登录 Azkaban
    async fn login(&self) -> Result<()> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or_else(|| anyhow::anyhow!("Client not initialized"))?;

        let login_url = format!("{}/?action=login", self.base_url);
        
        let response = client
            .post(&login_url)
            .form(&[
                ("username", &self.config.azkaban.username),
                ("password", &self.config.azkaban.password),
            ])
            .send()
            .await
            .context("Failed to send login request")?;

        if response.status().is_success() {
            let body = response.text().await.context("Failed to read response")?;
            
            // 检查是否登录成功
            if body.contains("Login failed") {
                anyhow::bail!("Azkaban login failed: invalid credentials");
            }

            tracing::info!("Logged in to Azkaban as {}", self.config.azkaban.username);
        } else {
            anyhow::bail!("Azkaban login failed with status: {}", response.status());
        }

        Ok(())
    }

    /// 上传项目
    pub async fn upload_project(
        &self,
        project_name: &str,
        zip_file_path: &str,
    ) -> Result<i64> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or_else(|| anyhow::anyhow!("Client not initialized"))?;

        let file_bytes = tokio::fs::read(zip_file_path)
            .await
            .context("Failed to read zip file")?;

        let upload_url = format!("{}/upload?project={}", self.base_url, project_name);

        let response = client
            .post(&upload_url)
            .header("Content-Type", "application/zip")
            .body(file_bytes)
            .send()
            .await
            .context("Failed to upload project")?;

        let body = response.text().await.context("Failed to read response")?;
        
        // 解析响应
        if body.contains("Error") {
            anyhow::bail!("Upload failed: {}", body);
        }

        tracing::info!("Project {} uploaded successfully", project_name);
        
        Ok(0)
    }

    /// 执行 flow
    pub async fn execute_flow(
        &self,
        project_name: &str,
        flow_name: &str,
        _params: Option<std::collections::HashMap<String, String>>,
    ) -> Result<i64> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or_else(|| anyhow::anyhow!("Client not initialized"))?;

        let mut form_data = vec![
            ("ajax", "executeFlow"),
            ("project", project_name),
            ("flow", flow_name),
        ];

        let response = client
            .post(format!("{}/executor", self.base_url))
            .form(&form_data)
            .send()
            .await
            .context("Failed to execute flow")?;

        let body = response.text().await.context("Failed to read response")?;
        
        // 解析执行 ID
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(exec_id) = json.get("execid").and_then(|v| v.as_i64()) {
                tracing::info!("Flow {} executed with exec_id: {}", flow_name, exec_id);
                return Ok(exec_id);
            }
        }

        anyhow::bail!("Failed to parse execution ID from response: {}", body);
    }

    /// 获取执行状态
    pub async fn get_execution_status(&self, exec_id: i64) -> Result<ExecStatus> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or_else(|| anyhow::anyhow!("Client not initialized"))?;

        let response = client
            .get(format!("{}/executor?ajax=getExecInfo&execid={}", self.base_url, exec_id))
            .send()
            .await
            .context("Failed to get execution status")?;

        let body = response.text().await.context("Failed to read response")?;
        
        // 解析状态
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(status) = json.get("status").and_then(|v| v.as_str()) {
                return match status.to_uppercase().as_str() {
                    "RUNNING" => Ok(ExecStatus::Running),
                    "SUCCEEDED" => Ok(ExecStatus::Succeeded),
                    "FAILED" => Ok(ExecStatus::Failed),
                    "KILLED" => Ok(ExecStatus::Killed),
                    _ => Ok(ExecStatus::Prepared),
                };
            }
        }

        anyhow::bail!("Failed to parse execution status: {}", body);
    }

    /// 获取执行日志
    pub async fn get_execution_logs(&self, exec_id: i64, job_id: &str) -> Result<String> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or_else(|| anyhow::anyhow!("Client not initialized"))?;

        let response = client
            .get(format!(
                "{}/executor?ajax=fetchJobLog&execid={}&jobId={}",
                self.base_url, exec_id, job_id
            ))
            .send()
            .await
            .context("Failed to get job logs")?;

        let logs = response.text().await.context("Failed to read logs")?;
        Ok(logs)
    }

    /// 取消执行
    pub async fn cancel_execution(&self, exec_id: i64) -> Result<()> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or_else(|| anyhow::anyhow!("Client not initialized"))?;

        let response = client
            .post(format!("{}/executor?ajax=cancelFlow&execid={}", self.base_url, exec_id))
            .send()
            .await
            .context("Failed to cancel execution")?;

        let _body = response.text().await.context("Failed to read response")?;
        
        tracing::info!("Execution {} cancelled", exec_id);
        Ok(())
    }

    /// 列出所有项目
    pub async fn list_projects(&self) -> Result<Vec<AzkabanProject>> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or_else(|| anyhow::anyhow!("Client not initialized"))?;

        let response = client
            .get(format!("{}/manager?ajax=fetchallprojects", self.base_url))
            .send()
            .await
            .context("Failed to list projects")?;

        let body = response.text().await.context("Failed to read response")?;
        
        // 解析项目列表
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(projects) = json.get("projects").and_then(|v| v.as_array()) {
                let result: Vec<AzkabanProject> = projects
                    .iter()
                    .filter_map(|p| {
                        Some(AzkabanProject {
                            name: p.get("name")?.as_str()?.to_string(),
                            description: p.get("description").and_then(|v| v.as_str()).map(String::from),
                        })
                    })
                    .collect();
                return Ok(result);
            }
        }

        anyhow::bail!("Failed to parse projects: {}", body);
    }

    /// 获取 flow 定义
    pub async fn get_flow_definition(&self, project_name: &str, flow_name: &str) -> Result<String> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or_else(|| anyhow::anyhow!("Client not initialized"))?;

        let response = client
            .get(format!(
                "{}/manager?project={}&ajax=getFlowInfo&flow={}",
                self.base_url, project_name, flow_name
            ))
            .send()
            .await
            .context("Failed to get flow definition")?;

        let body = response.text().await.context("Failed to read response")?;
        Ok(body)
    }

    /// 调度执行 (定时任务)
    pub async fn schedule_flow(
        &self,
        project_name: &str,
        flow_name: &str,
        cron_expression: &str,
    ) -> Result<()> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or_else(|| anyhow::anyhow!("Client not initialized"))?;

        let response = client
            .post(format!("{}/schedule", self.base_url))
            .form(&[
                ("ajax", "createSchedule"),
                ("projectName", project_name),
                ("flowName", flow_name),
                ("cronExpression", cron_expression),
            ])
            .send()
            .await
            .context("Failed to schedule flow")?;

        let _body = response.text().await.context("Failed to read response")?;
        
        tracing::info!("Flow {} scheduled with cron: {}", flow_name, cron_expression);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_azkaban_service_creation() {
        let config = AppConfig::default();
        let service = AzkabanService::new(config);
        
        assert!(service.client.read().await.is_none());
    }
}
