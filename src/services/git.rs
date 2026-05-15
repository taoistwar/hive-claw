//! Git 服务
//! 
//! 负责与 Git 仓库交互，管理任务代码的版本控制

use crate::config::AppConfig;
use anyhow::{Result, Context};
use git2::{Repository, Signature, StatusOptions, ResetType};
use std::path::{Path, PathBuf};

/// Git 服务
pub struct GitService {
    config: AppConfig,
    repo_path: PathBuf,
    repository: Option<Repository>,
}

impl GitService {
    /// 创建新的 Git 服务
    pub fn new(config: AppConfig) -> Self {
        let repo_path = PathBuf::from(&config.git.remote)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        Self {
            config,
            repo_path,
            repository: None,
        }
    }

    /// 初始化或打开仓库
    pub fn initialize(&mut self) -> Result<()> {
        // 检查仓库是否存在
        if self.repo_path.join(".git").exists() {
            self.repository = Some(
                Repository::open(&self.repo_path)
                    .context("Failed to open repository")?
            );
            tracing::info!("Opened existing repository at {:?}", self.repo_path);
        } else {
            // 克隆远程仓库
            self.clone_repository()?;
        }

        Ok(())
    }

    /// 克隆远程仓库
    fn clone_repository(&mut self) -> Result<()> {
        let repo_dir = self.repo_path.join("azkaban-jobs");
        
        if !repo_dir.exists() {
            Repository::clone(&self.config.git.remote, &repo_dir)
                .context("Failed to clone repository")?;
            
            tracing::info!("Cloned repository to {:?}", repo_dir);
        }

        self.repository = Some(Repository::open(&repo_dir)?);
        Ok(())
    }

    /// 获取当前分支
    pub fn current_branch(&self) -> Result<String> {
        let repo = self.get_repo()?;
        let head = repo.head().context("Failed to get HEAD")?;
        let branch_name = head.shorthand().unwrap_or("main").to_string();
        Ok(branch_name)
    }

    /// 创建新分支
    pub fn create_branch(&self, branch_name: &str) -> Result<()> {
        let repo = self.get_repo()?;
        
        // 获取当前 HEAD 的 commit
        let head = repo.head().context("Failed to get HEAD")?;
        let target = repo.find_commit(head.target().unwrap())
            .context("Failed to find commit")?;

        // 创建新分支
        repo.branch(branch_name, &target, false)
            .context("Failed to create branch")?;

        // 切换到新分支
        repo.set_head(format!("refs/heads/{}", branch_name).as_str())
            .context("Failed to set HEAD")?;

        // 检查工作目录
        let mut opts = git2::build::CheckoutBuilder::new();
        opts.force();
        repo.checkout_head(Some(&mut opts))
            .context("Failed to checkout")?;

        tracing::info!("Created and switched to branch: {}", branch_name);
        Ok(())
    }

    /// 切换分支
    pub fn checkout_branch(&self, branch_name: &str) -> Result<()> {
        let repo = self.get_repo()?;
        
        repo.set_head(format!("refs/heads/{}", branch_name).as_str())
            .or_else::<anyhow::Error, _>(|_| {
                // 如果是远程分支，先创建本地跟踪分支
                repo.set_head(format!("refs/remotes/origin/{}", branch_name).as_str())?;
                Ok(())
            })
            .context("Failed to set HEAD")?;

        let mut opts = git2::build::CheckoutBuilder::new();
        opts.force();
        repo.checkout_head(Some(&mut opts))
            .context("Failed to checkout")?;

        tracing::info!("Switched to branch: {}", branch_name);
        Ok(())
    }

    /// 添加文件
    pub fn add_file(&self, file_path: &str) -> Result<()> {
        let repo = self.get_repo()?;
        let mut index = repo.index().context("Failed to get index")?;
        
        index.add_path(Path::new(file_path))
            .context("Failed to add file")?;
        
        index.write().context("Failed to write index")?;

        tracing::debug!("Added file: {}", file_path);
        Ok(())
    }

    /// 添加所有更改
    pub fn add_all(&self) -> Result<()> {
        let repo = self.get_repo()?;
        let mut index = repo.index().context("Failed to get index")?;
        
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .context("Failed to add all files")?;
        
        index.write().context("Failed to write index")?;

        tracing::info!("Added all changes");
        Ok(())
    }

    /// 提交更改
    pub fn commit(&self, message: &str) -> Result<String> {
        let repo = self.get_repo()?;
        
        // 获取签名
        let signature = Signature::now(
            &self.config.git.username,
            &format!("{}@example.com", self.config.git.username),
        ).context("Failed to create signature")?;

        // 写入索引
        let mut index = repo.index().context("Failed to get index")?;
        index.write().context("Failed to write index")?;

        // 创建 tree
        let tree_id = index.write_tree().context("Failed to write tree")?;
        let tree = repo.find_tree(tree_id).context("Failed to find tree")?;

        // 获取父 commit
        let head = repo.head().context("Failed to get HEAD")?;
        let parent = repo.find_commit(head.target().unwrap())
            .context("Failed to find parent commit")?;

        // 创建提交
        let commit_id = repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&parent],
        ).context("Failed to create commit")?;

        tracing::info!("Created commit: {}", commit_id);
        Ok(commit_id.to_string())
    }

    /// 推送到远程
    pub fn push(&self, branch_name: &str, force: bool) -> Result<()> {
        let repo = self.get_repo()?;
        
        let mut remote = repo.find_remote("origin")
            .or_else(|_| repo.remote("origin", &self.config.git.remote))?;

        let mut callbacks = git2::RemoteCallbacks::new();
        callbacks.credentials(|_url, _username, allowed_types| {
            if allowed_types.contains(git2::CredentialType::SSH_KEY) {
                git2::Cred::ssh_key_from_agent("git")
            } else {
                git2::Cred::default()
            }
        });

        let mut opts = git2::PushOptions::new();
        opts.remote_callbacks(callbacks);

        let refspec = if force {
            format!("+refs/heads/{}:refs/heads/{}", branch_name, branch_name)
        } else {
            format!("refs/heads/{}:refs/heads/{}", branch_name, branch_name)
        };

        remote.push(&[refspec.as_str()], Some(&mut opts))
            .context("Failed to push")?;

        tracing::info!("Pushed branch {} to remote", branch_name);
        Ok(())
    }

    /// 拉取远程更改
    pub fn pull(&self) -> Result<()> {
        let repo = self.get_repo()?;
        
        let mut remote = repo.find_remote("origin")?;
        
        let mut callbacks = git2::RemoteCallbacks::new();
        callbacks.credentials(|_url, _username, allowed_types| {
            if allowed_types.contains(git2::CredentialType::SSH_KEY) {
                git2::Cred::ssh_key_from_agent("git")
            } else {
                git2::Cred::default()
            }
        });

        let mut opts = git2::FetchOptions::new();
        opts.remote_callbacks(callbacks);

        remote.fetch(&[self.config.git.branch_prefix.as_str()], Some(&mut opts), None)
            .context("Failed to fetch")?;

        // 合并远程分支
        let fetch_head = repo.find_reference("FETCH_HEAD")?;
        let fetch_commit = repo.reference_to_annotated_commit(&fetch_head)?;

        let analysis = repo.merge_analysis(&[&fetch_commit])?;

        if analysis.0.is_up_to_date() {
            tracing::info!("Repository is up to date");
            return Ok(());
        }

        if analysis.0.is_fast_forward() {
            // Fast forward merge
            let mut reference = repo.find_reference("refs/heads/master")?;
            reference.set_target(fetch_commit.id(), "Fast-forward")?;
            repo.checkout_head(Some(
                &mut git2::build::CheckoutBuilder::new().force()
            ))?;
            tracing::info!("Fast-forward merge completed");
        } else {
            // Normal merge - simplified
            tracing::warn!("Merge operation requires manual resolution in this version");
            return Ok(());
        }

        Ok(())
    }

    /// 创建 Pull Request（通过 GitHub API）
    pub async fn create_pull_request(
        &self,
        branch_name: &str,
        title: &str,
        _description: &str,
    ) -> Result<String> {
        // 这里需要调用 GitHub API
        // 使用 octocrab 或其他 GitHub API 客户端
        
        tracing::info!("Creating PR: {} from {}", title, branch_name);
        
        // Mock 实现
        Ok(format!("PR created: {}", title))
    }

    /// 获取仓库状态
    pub fn status(&self) -> Result<Vec<String>> {
        let repo = self.get_repo()?;
        let mut status_opts = StatusOptions::new();
        status_opts.include_ignored(false);

        let statuses = repo.statuses(Some(&mut status_opts))
            .context("Failed to get status")?;

        let mut changes = vec![];
        for entry in statuses.iter() {
            if let Some(path) = entry.path() {
                let status = entry.status();
                if status.is_index_new() {
                    changes.push(format!("A {}", path));
                } else if status.is_index_modified() {
                    changes.push(format!("M {}", path));
                } else if status.is_wt_new() {
                    changes.push(format!("?? {}", path));
                } else if status.is_wt_modified() {
                    changes.push(format!(" M {}", path));
                }
            }
        }

        Ok(changes)
    }

    /// 重置到指定 commit
    pub fn reset(&self, commit_id: &str, hard: bool) -> Result<()> {
        let repo = self.get_repo()?;
        let commit = repo.revparse_single(commit_id)
            .context("Failed to find commit")?;

        let reset_type = if hard {
            ResetType::Hard
        } else {
            ResetType::Soft
        };

        repo.reset(&commit, reset_type, None)
            .context("Failed to reset")?;

        tracing::info!("Reset to commit: {}", commit_id);
        Ok(())
    }

    /// 获取日志
    pub fn log(&self, count: usize) -> Result<Vec<String>> {
        let repo = self.get_repo()?;
        let mut revwalk = repo.revwalk().context("Failed to create revwalk")?;
        revwalk.push_head().context("Failed to push HEAD")?;

        let mut commits = vec![];
        for (i, oid_result) in revwalk.enumerate() {
            if i >= count {
                break;
            }
            let oid = oid_result.context("Failed to get OID")?;
            let commit = repo.find_commit(oid).context("Failed to find commit")?;
            commits.push(format!(
                "{} - {}",
                &oid.to_string()[..7],
                commit.summary().unwrap_or("No message")
            ));
        }

        Ok(commits)
    }

    fn get_repo(&self) -> Result<&Repository> {
        self.repository.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Git repository not initialized. Call initialize() first."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_git_service_creation() {
        let config = AppConfig::default();
        let service = GitService::new(config);
        
        assert!(service.repository.is_none());
    }
}
