# Research: 管理中心

**Created**: 2026-05-25
**Feature**: 管理中心 (003-admin-center)

## Technical Decisions

### 1. 认证方案

**Decision**: JWT Token + 刷新 Token 机制

**Rationale**:
- 无状态认证，后端无需存储会话，易于水平扩展
- Token 自包含用户信息，减少数据库查询
- 刷新 Token 机制平衡安全性和用户体验
- 符合 Rust + axum 生态最佳实践

**Alternatives Considered**:
- Session-based: 需要后端存储会话状态，增加复杂性
- Cookie-based: CSRF 防护复杂，跨域支持差

**Implementation**:
- Access Token: 有效期 24 小时，存储管理员 ID、角色、昵称
- 刷新 Token: 有效期 7 天，单独存储，用于刷新 Access Token
- Token 签名：使用 HS256 算法，密钥从环境变量读取

### 2. 密码加密方案

**Decision**: bcrypt

**Rationale**:
- 自适应成本因子，随硬件性能提升而调整
- Rust `bcrypt` crate 成熟稳定
- 抗彩虹表攻击
- 行业标准的密码哈希算法

**Alternatives Considered**:
- argon2: 更新更安全，但 Rust 生态相对较新
- scrypt: 内存密集型，适合特殊场景
- PBKDF2: 较老，计算成本较低

**Implementation**:
- bcrypt cost: 12（平衡安全性和性能）
- 密码长度：按 Unicode 字符计数 6-20，并至少包含一个 ASCII 字母和数字
- 盐值：bcrypt 自动生成

### 3. 数据库选择

**Decision**: MySQL 8.0+

**Rationale**:
- 生产环境标准，成熟稳定
- 支持并发访问和水平扩展
- InnoDB 引擎提供事务支持
- 完善的备份和恢复机制
- 符合企业级应用需求

**Alternatives Considered**:
- SQLite: 嵌入式，不适合并发访问
- PostgreSQL: 功能强大但运维复杂度略高
- MariaDB: MySQL 分支，兼容性良好

**Implementation**:
- 使用 SQLx 进行数据库操作（编译时 SQL 验证）
- 连接池配置：max_connections = 20
- 字符集：utf8mb4
- 引擎：InnoDB

### 4. 缓存选择

**Decision**: Redis 7+

**Rationale**:
- 高性能内存缓存，支持丰富数据结构
- 支持 TTL（过期时间），适合会话管理
- 持久化支持（RDB/AOF），可选数据持久化
- Rust 生态成熟（redis、bb8-redis crate）
- 符合宪法 v1.3.0 要求

**Alternatives Considered**:
- Memcached: 功能单一，不支持持久化
- 本地内存缓存：不支持分布式部署
- sled (嵌入式): 已被宪法弃用

**Implementation**:
- 使用 `redis` crate 或 `bb8-redis` 连接池
- 缓存策略：cache-aside（旁路缓存）
- 会话数据：必须设置 TTL（24 小时）
- 登录失败锁定：使用 Redis INCR + EXPIRE

### 5. 对象存储选择

**Decision**: Rustfs (S3 兼容)

**Rationale**:
- S3 协议兼容，生态丰富
- 支持本地部署（Rustfs）和云服务（AWS S3）
- Rust 支持良好（aws-sdk-s3, object_store）
- 适合文件上传、备份、静态资源
- 符合宪法 v1.3.0 要求

**Alternatives Considered**:
- 本地文件系统：不支持分布式，已被宪法禁止
- MinIO: S3 兼容，但 Rustfs 更轻量
- 云存储服务：锁定风险，运维复杂

**Implementation**:
- 使用 `aws-sdk-s3` crate 或 `object_store` crate
- 本地开发：使用 Rustfs 或 MinIO
- 生产环境：可切换到 AWS S3 或其他 S3 兼容服务
- 存储内容：管理员头像、导出文件等

### 6. 前端 UI 组件库

**Decision**: Ant Design

**Rationale**:
- 功能丰富的后台管理组件
- 内置表单验证、表格、布局组件
- TypeScript 支持良好
- 文档完善，社区活跃

**Alternatives Considered**:
- Material-UI: 设计语言不同，组件略少
- Chakra UI: 较新，生态不够成熟
- 原生 HTML: 开发效率低

**Implementation**:
- 使用 `antd` npm 包
- 表格组件：支持分页、排序、筛选
- 表单组件：内置验证规则
- 布局组件：响应式设计

### 5. 前端状态管理

**Decision**: React Hooks (useState, useContext, useReducer)

**Rationale**:
- 符合宪法 Principle V (Simplicity & YAGNI)
- 避免引入额外依赖（Redux、Zustand 等）
- 应用规模适中，hooks 足够管理
- 减少学习曲线

**Alternatives Considered**:
- Redux Toolkit: 过度复杂，样板代码多
- Zustand: 轻量但增加依赖
- Recoil: 较新，生态不成熟

**Implementation**:
- 认证状态：useContext + useReducer
- 表单状态：useState + controlled components
- API 数据：React Query (可选，视复杂度而定)

### 6. 登录失败锁定策略

**Decision**: 内存缓存 (tokio::sync::RwLock) + 数据库持久化

**Rationale**:
- 快速响应，无需查询数据库
- 重启后从数据库恢复锁定状态
- 实现简单，无额外依赖

**Alternatives Considered**:
- Redis: 增加外部依赖，过度复杂
- 纯数据库：每次登录需查询，性能较差
- 纯内存：重启后丢失锁定状态

**Implementation**:
- 内存缓存：`HashMap<手机号，LockInfo>`
- LockInfo: 失败次数、首次失败时间、锁定时间
- 定期清理过期锁定（每 10 分钟）
- 锁定信息异步持久化到数据库

### 7. 权限控制方案

**Decision**: 基于角色的访问控制 (RBAC) + 前端路由守卫 + 后端中间件验证

**Rationale**:
- 前后端双重验证，安全性高
- 前端即时反馈，用户体验好
- 后端最终验证，防止越权访问
- 角色定义清晰，易于扩展

**Alternatives Considered**:
- 纯后端验证：用户体验差，每次请求后才知无权限
- 纯前端验证：不安全，可被绕过
- 基于权限的细粒度控制：过度复杂

**Implementation**:
- 角色定义：Normal, System, Super
- 前端：React Router 守卫，根据角色过滤菜单
- 后端：axum 中间件，验证 Token 和角色
- 权限表：角色 -> 可访问菜单 -> 可执行操作

### 8. 前端构建工具

**Decision**: Vite

**Rationale**:
- 极速开发服务器启动
- 热模块替换 (HMR)
- 内置 TypeScript 支持
- 生产构建使用 Rollup，优化打包

**Alternatives Considered**:
- Create React App: 较慢，配置复杂
- Next.js: SSR 功能不必要，增加复杂度
- Parcel: 较新，生态不成熟

**Implementation**:
- 使用 `create-vite` 模板
- 配置 TypeScript 严格模式
- 配置 ESLint + Prettier
- 配置环境变量

## Best Practices

### Rust Backend

1. **项目结构**:
   - 按功能模块组织：models, services, api, middleware
   - 使用 `mod.rs` 明确导出
   - 错误处理使用 `thiserror` + `anyhow`

2. **数据库**:
   - 使用 SQLx 进行编译时 SQL 验证
   - 迁移脚本版本化管理
   - 连接池配置：max_connections = 10

3. **API 设计**:
   - RESTful 风格
   - 统一响应格式：`{ code, data, message }`
   - 错误码分类：1xxx-认证，2xxx-权限，3xxx-管理员管理

4. **日志**:
   - 使用 `tracing` + `tracing-subscriber`
   - 结构化日志（JSON 格式）
   - 敏感信息脱敏（密码、Token）

### React Frontend

1. **组件设计**:
   - 函数组件 + Hooks
   - 组件拆分：展示组件 + 逻辑组件
   - Props 类型定义清晰

2. **API 调用**:
   - 封装 axios 实例
   - 统一错误处理
   - 请求拦截器自动添加 Token

3. **表单验证**:
   - 前端即时验证（用户体验）
   - 后端最终验证（安全性）
   - 错误提示清晰明确

4. **样式**:
   - 使用 Ant Design 主题定制
   - 响应式布局
   - 统一颜色、间距变量

## Security Considerations

1. **密码安全**:
   - bcrypt 加密，cost=12
   - 密码按 Unicode 字符计数 6-20，并至少包含一个 ASCII 字母和数字
   - 不支持纯数字或纯字母密码

2. **Token 安全**:
   - HTTPS 传输（生产环境）
   - Token 签名密钥从环境变量读取
   - Token 不包含敏感信息

3. **输入验证**:
   - 手机号格式验证（11 位数字）
   - SQL 注入防护（参数化查询）
   - XSS 防护（前端转义）

4. **会话安全**:
   - Token 有效期 24 小时
   - 登出后 Token 失效（后端黑名单）
   - 登录失败锁定

5. **权限隔离**:
   - 前后端双重验证
   - 超级管理员操作记录日志
   - 敏感操作（删除、禁用）需二次确认

## Performance Optimization

1. **数据库**:
   - 手机号字段建立唯一索引
   - 登录记录表按时间分区（未来扩展）
   - 查询使用 LIMIT 限制返回行数

2. **API**:
   - 列表接口支持分页
   - 统计数据缓存（5 分钟）
   - 使用连接池

3. **前端**:
   - 路由懒加载
   - 组件按需加载
   - 表格虚拟滚动（大数据量时）

## Testing Strategy

1. **Backend**:
   - 单元测试：models, services
   - 集成测试：API endpoints
   - 使用 `cargo test`

2. **Frontend**:
   - 组件测试：Testing Library
   - 工具函数测试：Vitest
   - E2E 测试：Playwright (可选)

3. **Test Coverage**:
   - 目标：核心业务逻辑 > 80%
   - CI 强制要求：关键路径 100%
