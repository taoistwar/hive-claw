# Data Model: 管理中心

**Created**: 2026-05-25
**Feature**: 管理中心 (003-admin-center)

## Entities

### 1. Admin (管理员)

**Description**: 代表系统管理员用户，包含登录凭证和角色信息

**Fields**:
- `id`: i64 (主键，自增，从 1 开始)
- `phone`: String (唯一，11 位数字，索引)
- `nickname`: String (2-20 字符)
- `password_hash`: String (bcrypt 加密，60 字符)
- `role`: Enum (Normal=1, System=2, Super=3)
- `status`: Enum (Active=1, Disabled=0)
- `created_at`: DateTime (UTC，注册时间)
- `updated_at`: DateTime (UTC，最后更新时间)
- `last_login_at`: Option<DateTime> (UTC，最后登录时间，可为空)

**Constraints**:
- phone 唯一索引
- phone 格式：^1[3-9]\d{9}$ (中国大陆手机号)
- nickname 长度：2-20 字符
- password_hash: bcrypt 加密，cost=12
- 至少保留一个启用的 Super 管理员

**Relationships**:
- 一对多：Admin -> LoginRecord (一个管理员有多条登录记录)

**Validation Rules**:
- 手机号必须唯一
- 密码长度 6-20 位
- 角色只能是 Normal/System/Super
- 状态只能是 Active/Disabled
- 创建时间不可修改
- 最后登录时间由系统在登录成功时自动更新

### 2. LoginRecord (登录记录)

**Description**: 记录管理员登录历史，用于审计和仪表盘展示

**Fields**:
- `id`: i64 (主键，自增)
- `admin_id`: i64 (外键，关联 Admin.id)
- `login_at`: DateTime (UTC，登录时间)
- `ip_address`: String (IPv4/IPv6)
- `success`: Boolean (是否成功)
- `failure_reason`: Option<String> (失败原因，成功时为空)

**Constraints**:
- admin_id 外键约束（级联删除）
- login_at 索引（用于按时间查询）
- failure_reason 仅在 success=false 时有值

**Relationships**:
- 多对一：LoginRecord -> Admin (多条记录属于一个管理员)

**Validation Rules**:
- admin_id 必须存在于 Admin 表
- failure_reason 仅在登录失败时填写
- 失败原因枚举：WRONG_PASSWORD, ACCOUNT_DISABLED, ACCOUNT_LOCKED, OTHER

### 3. RolePermission (角色权限)

**Description**: 定义角色与权限的映射关系（硬编码，不持久化到数据库）

**Fields** (内存结构):
- `role`: Enum (Normal, System, Super)
- `accessible_menus`: Vec<String> (可访问的菜单列表)
- `allowed_operations`: Vec<String> (可执行的操作列表)

**Hardcoded Permissions**:

```rust
// 普通管理员
Normal: {
  accessible_menus: ["dashboard", "admin-list"],
  allowed_operations: ["view"]
}

// 系统管理员
System: {
  accessible_menus: ["dashboard", "admin-list", "admin-add", "admin-edit", "admin-toggle-status"],
  allowed_operations: ["view", "create", "update", "disable", "enable"]
}

// 超级管理员
Super: {
  accessible_menus: ["dashboard", "admin-list", "admin-add", "admin-edit", "admin-delete", "admin-toggle-status"],
  allowed_operations: ["view", "create", "update", "delete", "disable", "enable"]
}
```

## Database Schema

### SQL DDL (MySQL 8.0+)

```sql
-- 管理员表
CREATE TABLE admins (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    phone VARCHAR(11) NOT NULL UNIQUE,
    nickname VARCHAR(20) NOT NULL,
    password_hash VARCHAR(60) NOT NULL,
    role TINYINT NOT NULL DEFAULT 1 COMMENT '1=Normal, 2=System, 3=Super',
    status TINYINT NOT NULL DEFAULT 1 COMMENT '1=Active, 0=Disabled',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    last_login_at DATETIME DEFAULT NULL,
    INDEX idx_phone (phone)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='管理员表';

-- 登录记录表
CREATE TABLE login_records (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    admin_id BIGINT NOT NULL,
    login_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ip_address VARCHAR(45) NOT NULL COMMENT 'IPv4 or IPv6',
    success TINYINT(1) NOT NULL COMMENT '1=true, 0=false',
    failure_reason VARCHAR(50) DEFAULT NULL COMMENT 'WRONG_PASSWORD, ACCOUNT_DISABLED, etc.',
    INDEX idx_admin_id (admin_id),
    INDEX idx_login_at (login_at),
    CONSTRAINT fk_login_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='登录记录表';
```

## State Transitions

### Admin Status

```
Active (1) ──[禁用]──> Disabled (0)
   ▲                        │
   └──────[启用]────────────┘
```

**Constraints**:
- 禁用：需要确认，不能禁用最后一个 Super 管理员
- 启用：无限制
- 删除：不能删除 Super 管理员

### Login Flow

```
[未登录] ──[输入 credentials]──> [验证中]
    │                              │
    │                              ├──[失败]──> [显示错误]
    │                              │
    │                              └──[成功]──> [更新 last_login_at]
    │                                              │
    │                                              ──> [已登录]
    │
[已登录] ──[Token 过期]──> [未登录]
    │
    └──[登出]──> [未登录]
```

## Validation Rules

### 手机号验证
```rust
fn validate_phone(phone: &str) -> Result<()> {
    if !phone.matches(char::is_numeric).count() == 11 {
        return Err("手机号必须为 11 位数字");
    }
    if !phone.starts_with('1') || !matches!(phone.chars().nth(1).unwrap(), '3'..='9') {
        return Err("手机号格式不正确");
    }
    Ok(())
}
```

### 密码验证
```rust
fn validate_password(password: &str) -> Result<()> {
    if password.len() < 6 || password.len() > 20 {
        return Err("密码长度必须为 6-20 位");
    }
    // 可选：检查弱密码
    Ok(())
}
```

### 角色权限验证
```rust
fn check_permission(role: Role, operation: &str) -> bool {
    let permissions = get_role_permissions(role);
    permissions.allowed_operations.contains(&operation.to_string())
}
```

## Indexes

### admins 表
- `idx_admins_phone`: phone 字段唯一索引（登录查询）

### login_records 表
- `idx_login_records_admin_id`: admin_id 索引（查询某管理员的登录记录）
- `idx_login_records_login_at`: login_at 索引（按时间范围查询）

## Migrations

### V001__create_admins_table.sql
```sql
-- 创建 admins 表（见上方 DDL）
```

### V002__create_login_records_table.sql
```sql
-- 创建 login_records 表（见上方 DDL）
```

### V003__seed_super_admin.sql
```sql
-- 初始化超级管理员（通过脚本生成，非直接 SQL）
-- 实际数据由 create-super-admin 二进制文件插入
```

## MySQL Configuration

### 连接池配置
```rust
use sqlx::mysql::MySqlPoolOptions;

let pool = MySqlPoolOptions::new()
    .max_connections(20)
    .min_connections(5)
    .connect_timeout(Duration::from_secs(30))
    .idle_timeout(Duration::from_secs(600))
    .connect(&database_url)
    .await?;
```

### 数据库初始化
```bash
# 创建数据库
mysql -u root -p -e "CREATE DATABASE hiveweb CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"

# 创建用户（可选）
mysql -u root -p -e "CREATE USER 'hiveweb'@'localhost' IDENTIFIED BY 'password';"
mysql -u root -p -e "GRANT ALL PRIVILEGES ON hiveweb.* TO 'hiveweb'@'localhost';"
mysql -u root -p -e "FLUSH PRIVILEGES;"
```

## Rust Structs

### Admin Model
```rust
#[derive(Debug, Clone, FromRow)]
pub struct Admin {
    pub id: i64,
    pub phone: String,
    pub nickname: String,
    pub password_hash: String,
    pub role: Role,
    pub status: AdminStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, FromRow, sqlx::Type)]
#[sqlx(type_name = "INTEGER")]
#[repr(i32)]
pub enum Role {
    Normal = 1,
    System = 2,
    Super = 3,
}

#[derive(Debug, Clone, FromRow, sqlx::Type)]
#[sqlx(type_name = "INTEGER")]
#[repr(i32)]
pub enum AdminStatus {
    Disabled = 0,
    Active = 1,
}
```

### LoginRecord Model
```rust
#[derive(Debug, Clone, FromRow)]
pub struct LoginRecord {
    pub id: i64,
    pub admin_id: i64,
    pub login_at: chrono::DateTime<chrono::Utc>,
    pub ip_address: String,
    pub success: bool,
    pub failure_reason: Option<String>,
}
```

### DTOs (Data Transfer Objects)
```rust
// 请求 DTOs
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub phone: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateAdminRequest {
    pub phone: String,
    pub nickname: String,
    pub password: String,
    pub role: Role,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAdminRequest {
    pub nickname: Option<String>,
    pub phone: Option<String>,
    pub status: Option<AdminStatus>,
}

// 响应 DTOs
#[derive(Debug, Serialize)]
pub struct AdminResponse {
    pub id: i64,
    pub phone: String,
    pub nickname: String,
    pub role: Role,
    pub status: AdminStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub refresh_token: String,
    pub admin: AdminResponse,
}
```
