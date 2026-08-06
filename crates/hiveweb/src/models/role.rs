use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Normal = 1,
    System = 2,
    Super = 3,
}

impl Role {
    /// 仅查看（列表 / 详情）— spec §US3 AS-1：所有角色都可以看到管理员列表。
    /// 操作类按钮由 UI 按更细的 capability 隐藏；后端在 GET 上不应拒绝。
    pub fn can_view_admins(&self) -> bool {
        true
    }

    /// 写操作的总开关（创建 / 编辑 / 切换状态）— spec §US3 AS-2/AS-3。
    /// System 与 Super 都可以；Normal 不可。
    pub fn can_modify_admins(&self) -> bool {
        matches!(self, Role::System | Role::Super)
    }

    /// 删除管理员账号 — spec §US3 AS-3。仅 Super 可以。
    pub fn can_delete_admins(&self) -> bool {
        matches!(self, Role::Super)
    }

    /// 兼容旧 API：等价于 `can_modify_admins()`。新代码请直接使用更细的方法。
    #[deprecated(note = "Use can_view_admins / can_modify_admins / can_delete_admins instead")]
    pub fn can_manage_admins(&self) -> bool {
        self.can_modify_admins()
    }

    pub fn can_view_dashboard(&self) -> bool {
        true
    }

    /// Runtime audit details can contain operational metadata and are therefore
    /// restricted to Super administrators (004 spec TM-3).
    pub fn can_view_runtime_audit_logs(&self) -> bool {
        matches!(self, Role::Super)
    }

    pub fn can_modify_role(&self, target_role: &Role) -> bool {
        match self {
            Role::Super => true,
            Role::System => !matches!(target_role, Role::Super),
            Role::Normal => false,
        }
    }

    pub fn can_modify_target_admin(&self, target_role: &Role) -> bool {
        match self {
            Role::Super => true,
            Role::System => !matches!(target_role, Role::Super),
            Role::Normal => false,
        }
    }

    pub fn can_delete_target_admin(&self, target_role: &Role) -> bool {
        matches!(self, Role::Super) && !matches!(target_role, Role::Super)
    }

    pub fn can_toggle_target_admin_status(&self, target_role: &Role) -> bool {
        match self {
            Role::Super => true,
            Role::System => !matches!(target_role, Role::Super),
            Role::Normal => false,
        }
    }

    /// Permission check retained for the legacy recommended-game admin API.
    #[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
    pub fn can_manage_recommended_games(&self) -> bool {
        matches!(self, Role::System | Role::Super)
    }

    /// Delete permission retained for the legacy recommended-game admin API.
    #[deprecated(note = "Legacy recommended-game API; retained for compatibility only")]
    pub fn can_delete_recommended_games(&self) -> bool {
        matches!(self, Role::Super)
    }

    pub fn accessible_menus(&self) -> Vec<&str> {
        match self {
            Role::Normal => vec!["dashboard", "admins", "settings"],
            Role::System => vec!["dashboard", "admins"],
            Role::Super => vec!["dashboard", "admins", "settings"],
        }
    }

    pub fn allowed_operations(&self) -> Vec<&str> {
        match self {
            Role::Normal => vec!["view_dashboard", "view_profile"],
            Role::System => vec![
                "view_dashboard",
                "view_profile",
                "create_admin",
                "update_admin",
                "disable_admin",
                "enable_admin",
            ],
            Role::Super => vec![
                "view_dashboard",
                "view_profile",
                "create_admin",
                "update_admin",
                "delete_admin",
                "disable_admin",
                "enable_admin",
                "change_role",
                "system_settings",
            ],
        }
    }

    pub fn has_permission(&self, permission: &str) -> bool {
        let allowed = self.allowed_operations();
        allowed.contains(&permission)
    }
}

impl TryFrom<i8> for Role {
    type Error = anyhow::Error;

    fn try_from(value: i8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Role::Normal),
            2 => Ok(Role::System),
            3 => Ok(Role::Super),
            _ => Err(anyhow::anyhow!("Invalid role value: {}", value)),
        }
    }
}

impl From<Role> for i8 {
    fn from(role: Role) -> Self {
        role as i8
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Normal => write!(f, "Normal"),
            Role::System => write!(f, "System"),
            Role::Super => write!(f, "Super"),
        }
    }
}
