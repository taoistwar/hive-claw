//! AI 服务
//! 
//! 负责与 GPT-4 API 交互，提供 SQL 生成、代码审查、错误修复等功能

use crate::config::AppConfig;
use anyhow::{Result, Context};
use reqwest::Client;
use serde::{Serialize, Deserialize};

/// AI 提供商
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AIProvider {
    OpenAI,
    Anthropic,
    Azure,
}

/// AI 模型配置
#[derive(Debug, Clone)]
pub struct AIModel {
    pub provider: AIProvider,
    pub model_name: String,
    pub api_key: String,
    pub api_base: Option<String>,
}

/// 对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// AI 服务
pub struct AIService {
    config: AppConfig,
    client: Client,
    model: AIModel,
}

impl AIService {
    /// 创建新的 AI 服务
    pub fn new(config: AppConfig) -> Self {
        let model = AIModel {
            provider: AIProvider::OpenAI,
            model_name: config.ai.model.clone(),
            api_key: config.ai.api_key.clone(),
            api_base: None,
        };

        Self {
            client: Client::new(),
            model,
            config,
        }
    }

    /// 生成 SQL
    pub async fn generate_sql(
        &self,
        task_name: &str,
        template_type: &str,
        source_tables: &[String],
        target_table: &str,
        requirements: &str,
    ) -> Result<String> {
        let prompt = format!(
            r#"你是一个大数据 SQL 专家。请根据以下要求生成 Hive SQL 查询：

任务名称：{}
任务模板：{}
源表：{}
目标表：{}
需求描述：{}

请生成符合以下要求的 SQL：
1. 使用 Hive SQL 语法
2. 包含适当的注释
3. 处理分区字段 (dt)
4. 考虑性能优化（使用合适的连接和聚合策略）
5. 添加数据质量检查

只返回 SQL 代码，不要有其他解释。"#,
            task_name,
            template_type,
            source_tables.join(", "),
            target_table,
            requirements,
        );

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "你是一个专业的大数据 SQL 工程师，精通 Hive、Spark SQL 和数据仓库开发。".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ];

        self.chat(messages).await
    }

    /// 审查 SQL 质量
    pub async fn review_sql(&self, sql: &str) -> Result<Vec<String>> {
        let prompt = format!(
            r#"请审查以下 Hive SQL 代码的质量，并列出所有问题和改进建议：

```sql
{}
```

请从以下维度进行审查：
1. 语法正确性
2. 性能优化（JOIN 策略、分区剪枝、数据倾斜）
3. 代码规范（命名、注释、格式化）
4. 数据质量（NULL 处理、边界情况）
5. 可维护性

按严重程度列出所有问题（阻塞 > 严重 > 一般 > 建议）。"#,
            sql,
        );

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "你是一个资深数据仓库工程师，擅长 SQL 代码审查和性能优化。".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ];

        let response = self.chat(messages).await?;
        
        // 解析响应，提取问题列表
        let issues = response.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|s| s.to_string())
            .collect();

        Ok(issues)
    }

    /// 生成错误修复建议
    pub async fn suggest_fix(&self, error_message: &str, sql: &str) -> Result<String> {
        let prompt = format!(
            r#"SQL 执行失败，请分析错误并提供修复方案：

错误信息：
{}

原始 SQL：
```sql
{}
```

请提供：
1. 错误原因分析
2. 修复后的 SQL 代码
3. 预防类似问题的建议

只返回修复后的 SQL 代码。"#,
            error_message,
            sql,
        );

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "你是一个 SQL 调试专家，擅长分析错误并提供修复方案。".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ];

        self.chat(messages).await
    }

    /// 生成任务描述
    pub async fn generate_description(
        &self,
        task_name: &str,
        template_type: &str,
        source_tables: &[String],
        target_table: &str,
    ) -> Result<String> {
        let prompt = format!(
            r#"请为以下 Azkaban 任务生成简洁清晰的描述（50 字以内）：

任务名称：{}
任务模板：{}
源表：{}
目标表：{}

描述应该说明任务的目的、数据来源和用途。"#,
            task_name,
            template_type,
            source_tables.join(", "),
            target_table,
        );

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "你是一个技术文档写作者，擅长编写简洁清晰的技术描述。".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ];

        self.chat(messages).await
    }

    /// 通用对话接口
    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<String> {
        match self.model.provider {
            AIProvider::OpenAI => self.chat_openai(messages).await,
            AIProvider::Anthropic => self.chat_anthropic(messages).await,
            AIProvider::Azure => self.chat_azure(messages).await,
        }
    }

    /// OpenAI API 调用
    async fn chat_openai(&self, messages: Vec<ChatMessage>) -> Result<String> {
        let api_url = self.model.api_base
            .as_ref()
            .unwrap_or(&"https://api.openai.com/v1".to_string())
            .clone();

        let request_body = serde_json::json!({
            "model": self.model.model_name,
            "messages": messages,
            "temperature": 0.7,
            "max_tokens": 2000,
        });

        let response = self.client
            .post(format!("{}/chat/completions", api_url))
            .header("Authorization", format!("Bearer {}", self.model.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to OpenAI")?;

        if !response.status().is_success() {
            let error = response.text().await?;
            anyhow::bail!("OpenAI API error: {}", error);
        }

        let json: serde_json::Value = response.json().await.context("Failed to parse response")?;
        
        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid response format"))?;

        Ok(content.to_string())
    }

    /// Anthropic API 调用
    async fn chat_anthropic(&self, messages: Vec<ChatMessage>) -> Result<String> {
        let api_url = "https://api.anthropic.com/v1";
        
        let system_message = messages.iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .unwrap_or("");

        let user_messages: Vec<_> = messages.iter()
            .filter(|m| m.role == "user")
            .collect();

        let request_body = serde_json::json!({
            "model": self.model.model_name,
            "system": system_message,
            "messages": user_messages.iter().map(|m| {
                serde_json::json!({
                    "role": "user",
                    "content": m.content
                })
            }).collect::<Vec<_>>(),
            "max_tokens": 2000,
        });

        let response = self.client
            .post(format!("{}/messages", api_url))
            .header("x-api-key", &self.model.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Anthropic")?;

        if !response.status().is_success() {
            let error = response.text().await?;
            anyhow::bail!("Anthropic API error: {}", error);
        }

        let json: serde_json::Value = response.json().await.context("Failed to parse response")?;
        
        let content = json["content"][0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid response format"))?;

        Ok(content.to_string())
    }

    /// Azure OpenAI API 调用
    async fn chat_azure(&self, messages: Vec<ChatMessage>) -> Result<String> {
        let api_url = self.model.api_base
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Azure API base URL is required"))?;

        let request_body = serde_json::json!({
            "messages": messages,
            "temperature": 0.7,
            "max_tokens": 2000,
        });

        let response = self.client
            .post(format!("{}/chat/completions?api-version=2024-02-15-preview", api_url))
            .header("api-key", &self.model.api_key)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Azure")?;

        if !response.status().is_success() {
            let error = response.text().await?;
            anyhow::bail!("Azure API error: {}", error);
        }

        let json: serde_json::Value = response.json().await.context("Failed to parse response")?;
        
        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid response format"))?;

        Ok(content.to_string())
    }

    /// 生成数据质量规则
    pub async fn generate_quality_rules(&self, table_metadata: &str) -> Result<Vec<String>> {
        let prompt = format!(
            r#"根据以下表结构，生成数据质量检查规则：

{}

请为每个字段生成适当的质量检查规则，包括：
1. 非空检查
2. 唯一性检查（主键）
3. 值域检查
4. 格式检查
5. 一致性检查

返回规则列表，每行一条规则。"#,
            table_metadata,
        );

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "你是一个数据质量专家，擅长设计数据质量检查规则。".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ];

        let response = self.chat(messages).await?;
        
        let rules = response.lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
            .map(|s| s.to_string())
            .collect();

        Ok(rules)
    }

    /// 生成 ETL 文档
    pub async fn generate_etl_documentation(
        &self,
        task_name: &str,
        sql: &str,
        source_tables: &[String],
        target_table: &str,
    ) -> Result<String> {
        let prompt = format!(
            r#"请为以下 ETL 任务生成技术文档：

任务名称：{}
源表：{}
目标表：{}
SQL 代码：
```sql
{}
```

文档应包含：
1. 任务概述（目的、输入、输出）
2. 数据处理逻辑
3. 调度策略
4. 依赖关系
5. 数据质量要求
6. 常见问题排查

使用 Markdown 格式。"#,
            task_name,
            source_tables.join(", "),
            target_table,
            sql,
        );

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "你是一个技术文档工程师，擅长编写清晰的 ETL 文档。".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ];

        self.chat(messages).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ai_service_creation() {
        let config = AppConfig::default();
        let service = AIService::new(config);
        
        // Just verify service is created
        assert_eq!(service.model.provider, AIProvider::OpenAI);
    }
}
