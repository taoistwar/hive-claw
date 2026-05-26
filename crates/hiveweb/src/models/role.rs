use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Normal = 1,
    System = 2,
    Super = 3,
}

impl Role {
    pub fn can_manage_admins(&self) -> bool {
        matches!(self, Role::System | Role::Super)
    }

    pub fn can_view_dashboard(&self) -> bool {
        true
    }

    pub fn can_modify_role(&self, target_role: &Role) -> bool {
        match self {
            Role::Super => true,
            Role::System => !matches!(target_role, Role::Super),
            Role::Normal => false,
        }
    }

    pub fn accessible_menus(&self) -> Vec<&str> {
        match self {
            Role::Normal => vec!["dashboard"],
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePermission {
    pub role: Role,
    pub permissions: HashSet<String>,
}

impl RolePermission {
    pub fn for_role(role: Role) -> Self {
        let permissions: HashSet<String> = role
            .allowed_operations()
            .into_iter()
            .map(String::from)
            .collect();

        Self { role, permissions }
    }

    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.contains(permission)
    }

    pub fn can_manage_role(&self, target_role: Role) -> bool {
        self.role.can_modify_role(&target_role)
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
