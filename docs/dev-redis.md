# Redis 开发指南

本文说明 `hiveweb` 支持的 Redis 连接模式、认证规则、运行期行为，以及当前代码中
真正使用 Redis 的位置和目的。Redis 连接环境变量见
[环境变量](env-vars.md)，生产部署说明见 [生产安装](prod/install.md)。

Redis 当前是 `hiveweb` 的核心启动依赖：服务启动时必须完成连接和 `PING`，否则
进程不会开始监听 HTTP 端口。统一客户端实现在
`crates/hiveweb/src/cache/redis.rs`，业务代码应使用该模块提供的 `RedisClient`，
不要自行根据 `REDIS_URL` 创建 `redis::Client`，否则会绕过 Sentinel 支持。
项目使用 Redis 7+ 作为开发和部署基线。

## 功能边界

Redis 当前用于：

- 管理员登录失败计数和临时锁定；
- AI 助手每日使用次数；
- 外部数据库查询及 Agent 运行时内容的性能缓存；
- 启动连接检查和 `/health/ready` readiness 检查。

Redis 当前不用于保存 JWT、管理员登录态、聊天会话或聊天消息。这些数据分别采用
无状态 JWT 或 MySQL。通用 HTTP 限流和 SSE 并发计数也是进程内状态，不存入
Redis。因此，多实例部署时不要把所有“限流”行为都理解为 Redis 共享状态。

## 支持的连接模式

| 模式 | 使用场景 | 主要配置 | 说明 |
|------|----------|----------|------|
| `direct` | 本地开发、单 Redis 实例 | `REDIS_URL` | 默认模式，直接连接 URL 指定的节点 |
| `sentinel` | 由 Redis Sentinel 管理主从切换的环境 | `REDIS_SENTINEL_MASTER`、`REDIS_SENTINEL_NODES` | 通过 Sentinel 发现并连接 master |

目前不支持 Redis Cluster，也不从 replica 读取数据。当前构建的 Direct 和
Sentinel 模式都只支持普通 TCP：Direct 模式应使用 `redis://`，Sentinel 模式会
在解析配置时直接拒绝 `rediss://` 节点。Spring Lettuce 的
`max-active`、`max-idle`、`min-idle` 等连接池参数没有对应的 hiveweb 配置；
当前客户端使用 redis-rs 的异步 multiplexed connection 接口。

所有模式都支持：

```dotenv
# 获取 Redis 连接的总超时，必须大于 0
REDIS_CONNECT_TIMEOUT_MS=5000
```

### Direct 模式

`REDIS_MODE` 未设置时默认使用 Direct。地址、数据库索引和认证信息都写在
`REDIS_URL` 中：

```dotenv
REDIS_MODE=direct
REDIS_URL=redis://127.0.0.1:6379/0
REDIS_CONNECT_TIMEOUT_MS=5000
```

只使用密码和数据库 15：

```dotenv
REDIS_URL=redis://:<data-node-password>@127.0.0.1:6379/15
```

使用 Redis ACL 用户名和密码：

```dotenv
REDIS_URL=redis://<username>:<password>@127.0.0.1:6379/15
```

用户名或密码包含 URL 特殊字符时必须进行百分号编码。Direct 模式不读取
`REDIS_DATABASE`、`REDIS_USERNAME` 或 `REDIS_PASSWORD`，并忽略所有 Sentinel
专用变量。

### Sentinel 模式

最小配置如下。示例只有一份数据节点密码，因此不需要填写任何用户名，也不需要
填写 Sentinel 密码：

```dotenv
REDIS_MODE=sentinel
REDIS_SENTINEL_MASTER=mymaster
REDIS_SENTINEL_NODES=10.0.0.1:26379,10.0.0.2:26379,10.0.0.3:26379
REDIS_DATABASE=15
REDIS_PASSWORD=<data-node-password>
REDIS_CONNECT_TIMEOUT_MS=5000
REDIS_SENTINEL_REFRESH_MS=1000
```

Sentinel 节点支持 `host:port` 或 `redis://host:port`，多个节点使用英文逗号分隔。
Sentinel 模式忽略 `REDIS_URL`。

数据节点使用具名 ACL 用户时，同时配置：

```dotenv
REDIS_USERNAME=<data-node-acl-user>
REDIS_PASSWORD=<data-node-password>
```

只有 Sentinel 服务自身启用了认证时，才配置：

```dotenv
# Sentinel 只有密码、使用 default 用户时，只设置 PASSWORD
REDIS_SENTINEL_PASSWORD=<sentinel-password>

# Sentinel 使用具名 ACL 用户时，再同时设置 USERNAME
REDIS_SENTINEL_USERNAME=<sentinel-acl-user>
```

认证信息分为两组，不会自动复用：

| 配置 | 连接目标 | 默认值 |
|------|----------|--------|
| `REDIS_USERNAME`、`REDIS_PASSWORD` | Sentinel 返回的数据 master | 未设置 |
| `REDIS_SENTINEL_USERNAME`、`REDIS_SENTINEL_PASSWORD` | Sentinel 服务 | 未设置 |

用户名没有默认字符串。只设置密码时使用兼容 `requirepass` 的单参数 `AUTH`，
对应 Redis 7 的 `default` 用户；使用具名 ACL 用户时必须同时设置用户名和密码。
只设置用户名而没有密码不会触发认证。空字符串按“未设置”处理。

当前 Sentinel 模式仅支持普通 TCP，不接受 `rediss://`。Sentinel 返回的 master
地址必须能从 hiveweb 所在主机或容器访问；容器、NAT 环境还需检查 Redis 和
Sentinel 的 announce 地址及端口。

## Sentinel 发现与故障切换

Sentinel 模式的运行过程如下：

1. hiveweb 启动时向 Sentinel 查询 master，并用 `ROLE` 验证数据节点确实为
   master。
2. 成功发现后连接数据节点并执行 `PING`；发现、验证或 `PING` 失败都会终止
   hiveweb 启动。
3. 运行期缓存最近一次成功发现的 master，不会在每条 Redis 命令前查询
   Sentinel。
4. master 刷新不是独立后台任务，而是在业务获取 Redis 连接时，根据
   `REDIS_SENTINEL_REFRESH_MS` 懒触发。
5. 并发请求中只有一个请求负责刷新；其他请求继续使用缓存 master，不排队等待
   Sentinel。
6. 单次运行期刷新最多等待
   `min(REDIS_CONNECT_TIMEOUT_MS / 2, 1000ms)`。刷新失败或超时时继续使用缓存
   master，并在下一个刷新周期后重试。

客户端优先访问上次成功响应的 Sentinel，其他 Sentinel 节点以短延迟并发兜底。
发现 master 地址变化时会写入结构化日志。

Redis 连接或命令失败后不会自动重放命令，避免 `INCR`、`DECR` 等命令重复执行。
因此，master 刚切换且尚未触发下一次刷新时，少量请求仍可能失败；这属于当前实现
的故障切换边界。

## 当前使用 Redis 的位置

### 计数与安全状态

这些数据不是普通性能缓存，Redis 故障可能直接影响对应业务。

| 目的 | Key / 值 | TTL 与命令 | 主要代码位置 | 故障行为 |
|------|----------|------------|--------------|----------|
| 管理员登录防爆破 | `login_failed:{phone}` / 失败次数 | `INCR`；第 5 次失败时 `EXPIRE 900`；成功后 `DEL` | `services/auth.rs`、`api/auth.rs` | 登录或改密开始时无法检查锁定状态会返回服务不可用；部分清理和辅助计数路径为 best-effort |
| AI 助手每日配额 | `assistant:daily:{user_id}` / 当日已用次数 | `INCR`；首次计数设置到下一次 UTC 重置时刻的 TTL；超限或后续业务失败时 `DECR` | `api/chat_assistant.rs`；`api/recommended_game.rs` 只读 | `/api/assistant` 无法连接或 `INCR` 失败时拒绝请求；quota/usage 展示读取失败时按 0 处理 |

管理员登录计数不足 5 次时当前没有 TTL；第 5 次失败后才进入 15 分钟锁定期。
AI 助手配额的重置小时来自外部 `cc_config`，默认是 UTC 0 点，计算出的 TTL 最低
为 60 秒。首次设置 TTL 和错误路径中的 `DECR` 当前是 best-effort。

登录失败阈值和锁定时间目前在 `services/auth.rs` 中固定为 5 次和 900 秒，代码不
读取 `.env.example` 中同名含义的 `LOGIN_LOCK_MAX_ATTEMPTS`、
`LOGIN_LOCK_DURATION_SEC`；开发时不要依赖这两个变量改变行为。

### Cache-aside 性能缓存

通用实现在 `services/cache_helper.rs`：

```text
Redis GET -> 未命中或读取失败 -> 查询数据库 -> Redis SETEX -> 返回数据库结果
```

Redis 读写失败不会单独阻断这些查询；数据库仍是事实源。缓存值使用 JSON
序列化。

| 目的 | Key | TTL | 当前调用位置 |
|------|-----|-----|--------------|
| 外部 cloud_user 信息及用户存在校验 | `cloud_user:info:{user_id}` | 900 秒 | `services/membership.rs`；assistant、quota、newsession、recommended-game API |
| Agent 运行时完整内容，减少每个 hop 的多条 SQL | `agent:content:{agent_id}` | 默认 300 秒；`AGENT_CACHE_TTL_SECS` 可覆盖 | `services/agent.rs`、`runtime/orchestrator.rs` |
| 管理端外部游戏选项列表 | `external_games:list` | 1800 秒 | `api/game.rs` 的受保护外部游戏列表接口 |
| game_info 分类所需的游戏标签 | `game_tags:cc_game_tag_type1` | 600 秒 | `runtime/builtins/game_info.rs` 的 LLM 分类路径 |

`cloud_user:info` 会缓存“用户不存在”的 `None` 结果。`agent:content` 包含 prompt、
skills、tools、permissions、children 和 hooks；Agent 相关写路径会 best-effort
执行 `DEL`。如果失效失败，旧配置可能一直保留到 TTL 到期。

### 启动与健康检查

- `main.rs` 通过 `cache::redis::create_from_env()` 初始化统一客户端；连接或启动
  `PING` 失败时进程退出。
- `/health/live` 只检查进程存活，不访问 Redis。
- `/health/ready` 并行检查 MySQL 和 Redis，对 Redis 执行 `PING`；单项检查超时
  为 2 秒，失败时返回 HTTP 503 和 `not_ready`。

`RedisClient` 还会从 `AppState` 传入 orchestrator、hook、workflow 和 builtin
上下文。仅仅持有或透传客户端不代表这些模块都实际读写 Redis。

## 已定义但尚未接入的缓存

以下包装函数或常量已经存在，但当前没有生产调用点，不能视为已经启用：

- `services/membership.rs` 中 coins、disk、subscriptions、duration cards 的缓存
  包装；`query_balance` 当前仍直接查询外部数据库。
- `services/game_service.rs` 中单游戏详情和按渠道/客户端列出游戏的缓存包装；当前
  对应生产路径使用非缓存查询。
- `KEY_VIP_STATUS`、`KEY_CONFIG_PREFIX`、
  `KEY_AI_ASSISTANT_CHAT_LIMIT_CONFIG` 及其 TTL 常量；VIP 状态和 AI 助手限流配置
  当前直接查询外部数据库。

新增功能时应以实际调用链为准，不要仅根据 `cache_helper.rs` 中存在某个 key 常量
就假定缓存已经生效。

## 开发约定

- 业务代码统一接收 `crate::cache::redis::RedisClient`，不要创建原始
  `redis::Client`。
- 普通查询缓存优先使用 `cached_or_fetch`，并保持数据库为事实源；计数器、锁和
  需要原子语义的数据应显式使用 Redis 命令。
- 新 key 应包含稳定的业务前缀和唯一标识，并设置明确 TTL；不要保存密码、JWT、
  完整手机号等不必要的敏感数据。现有 `login_failed:{phone}` 属于历史 key，排障
  时不要把完整 key 输出到日志或工单。
- 缓存结构发生不兼容变更时，应更换 key 版本或安排失效，避免旧 JSON 导致反序列化
  错误。
- 修改数据库事实源后，如果对应数据有长 TTL 缓存，应在写路径 best-effort 删除
  缓存。
- 当前 key 没有统一的环境前缀。开发、测试和生产应使用不同 Redis 实例或至少不同
  database，避免跨环境 key 冲突。
- 不要在共享 Redis 上执行 `FLUSHDB` 或 `FLUSHALL`；排障和测试应按 key 前缀扫描
  并删除明确范围内的数据。

## 本地开发与排查

启动一个仅供本地开发使用的密码 Redis：

```bash
docker run --rm -d \
  --name hive-claw-redis-dev \
  -p 16379:6379 \
  redis:7-alpine \
  redis-server --requirepass dev-only-password
```

配置 hiveweb：

```dotenv
REDIS_MODE=direct
REDIS_URL=redis://:dev-only-password@127.0.0.1:16379/0
REDIS_CONNECT_TIMEOUT_MS=5000
```

检查数据节点、readiness 和常用 key：

```bash
REDISCLI_AUTH=dev-only-password redis-cli -h 127.0.0.1 -p 16379 PING
curl -i http://127.0.0.1:3300/health/ready

REDISCLI_AUTH=dev-only-password redis-cli -h 127.0.0.1 -p 16379 --scan \
  --pattern 'assistant:daily:*'
REDISCLI_AUTH=dev-only-password redis-cli -h 127.0.0.1 -p 16379 \
  TTL 'assistant:daily:1006419'
```

检查 Sentinel 当前报告的 master：

```bash
SENTINEL_HOST=sentinel.example.com
redis-cli -h "$SENTINEL_HOST" -p 26379 \
  SENTINEL get-master-addr-by-name mymaster
```

Sentinel 自身需要密码时，为这条命令单独提供 Sentinel 认证信息，不要误用数据节点
密码。排查顺序建议为：Sentinel 是否可达、master 名称是否正确、返回地址能否从
hiveweb 主机访问、数据节点认证是否正确、`ROLE` 是否返回 master、最后检查
`/health/ready` 和 hiveweb 日志。

相关配置与故障降级测试：

```bash
cargo test -p hiveweb --test contract_redis_config
cargo test -p hiveweb --test contract_health
```
