# API 文档

## 对外 API

以下 API 无需 JWT 鉴权，对所有外部调用方开放。

---

### 1. 获取热门推荐游戏

```
GET /api/recommended-games/top
```

获取按排序值降序排列的热门推荐游戏列表。

**鉴权：** 无需鉴权。

**查询参数：**

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `n` | `i64` | 是 | 返回数量，取值范围 [1, 10] |

**示例请求：**

```bash
curl -X GET "http://localhost:3300/api/recommended-games/top?n=5"
```

**成功响应 `200 OK`：**

```json
{
  "code": 0,
  "data": [
    {
      "name": "热门推荐标题",
      "reply": "推荐回复内容",
      "reason": "推荐理由",
      "tag": "标签",
      "game_category": "游戏分类",
      "game_image": "https://example.com/image.png",
      "game_id": "game_001",
      "game_name": "游戏名称"
    }
  ],
  "message": "ok"
}
```

**字段说明：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | `String` | 推荐标题 |
| `reply` | `String` | 推荐回复内容 |
| `reason` | `Option<String>` | 推荐理由 |
| `tag` | `Option<String>` | 标签 |
| `game_category` | `Option<String>` | 游戏分类 |
| `game_image` | `Option<String>` | 游戏封面图片 URL |
| `game_id` | `String` | 关联游戏 ID |
| `game_name` | `String` | 关联游戏名称 |

---

### 2. AI 助手对话

```
POST /api/assistant
```

向 AI 助手发送消息并获取回复。

**鉴权：** MD5 签名鉴权。

**鉴权方式：**

请求需要附带 `sign` 查询参数，其值为：

```
MD5(ASSISTANT_SECRET + "/api/assistant" + "?body=" + 请求体 JSON 字符串)
```

> 若环境变量 `ASSISTANT_SECRET` 为空，则跳过签名校验。

**请求头：**

| Header | 值 |
|--------|-----|
| `Content-Type` | `application/json; charset=UTF-8` |

**请求体：**

```json
{
  "user_id": 12345,
  "message": "你好，请推荐一款游戏",
  "channel": "app",
  "platform": "android",
  "app_version": "1.0.0"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `user_id` | `i64` | 是 | 用户 ID，必须大于 0 |
| `message` | `String` | 是 | 用户消息内容，不能为空或全空白 |
| `channel` | `String` | 是 | 渠道标识（如 `app`、`web`、`api`） |
| `platform` | `String` | 是 | 客户端平台（`android`、`iphone`、`ipad`、`web`） |
| `app_version` | `String` | 是 | 客户端版本号 |

**限流策略：**

- VIP 用户：默认 **50 次/天**
- 普通用户：默认 **5 次/天**
- 可通过后台 `global_config` 的 `vip_ask_times` / `normal_ask_times` 动态配置
- 同一用户同时只能有一个活跃会话（SSE 并发守卫）

**示例请求：**

```bash
BODY='{"user_id":12345,"message":"你好","channel":"app","platform":"android","app_version":"1.0.0"}'
SIGN=$(echo -n "${ASSISTANT_SECRET}/api/assistant?body=${BODY}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/assistant?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
```

**成功响应 `200 OK`：**

```json
{
  "reply": "您好！以下是为您推荐的游戏...",
  "elapsed_ms": 1523,
  "extensions": []
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `reply` | `String` | AI 助手的完整回复文本 |
| `elapsed_ms` | `Option<u64>` | LLM 处理耗时（毫秒） |
| `extensions` | `Option<Vec<Value>>` | AgentContext 扩展数据（可选） |

**错误响应：**

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（`user_id` ≤ 0、`message` 为空等） |
| `401` | MD5 签名校验失败 |
| `403` | 外部用户不存在 |
| `429` | 当日调用次数超限 |
| `409` | 同一用户已有活跃会话 |

**错误回滚：** 当 LLM 执行过程中出现错误时，系统会自动回滚当日的配额计数器，确保不会因失败而消耗用户配额。

---

### 3. 获取用户历史消息

```
POST /api/messages?sign={md5}
```

获取指定时间之前的最近 10 条用户聊天记录。

**鉴权：** MD5 签名鉴权。

**鉴权方式：**

请求 URL 中需要附带 `sign` 查询参数，其值为：

```
MD5(ASSISTANT_SECRET + "/api/messages?body=" + json_body)
```

> 若环境变量 `ASSISTANT_SECRET` 为空，则跳过签名校验。

**请求头：**

| Header | 值 |
|--------|-----|
| `Content-Type` | `application/json; charset=UTF-8` |

**请求体：**

```json
{
  "user_id": 12345,
  "date": "2026-06-03 14:30:00"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `user_id` | `i64` | 是 | 用户 ID，必须大于 0 |
| `date` | `String` | 是 | 最后一条聊天记录的时间，格式 `YYYY-MM-DD HH:MM:SS`，返回该时间之前的最近 10 条 |

**URL 查询参数：**

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `sign` | `String` | 否* | MD5 签名（`ASSISTANT_SECRET` 非空时必填） |

**示例请求：**

```bash
USER_ID=12345
DATE="2026-06-03 14:30:00"
BODY="{\"user_id\":${USER_ID},\"date\":\"${DATE}\"}"
SIGN_STR="/api/messages?body=${BODY}"
SIGN=$(echo -n "${ASSISTANT_SECRET}${SIGN_STR}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/messages?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
```

**成功响应 `200 OK`：**

```json
{
  "messages": [
    {
      "id": 100,
      "session_id": 5,
      "user_id": 12345,
      "role": "assistant",
      "content": "您好！以下是为您推荐的游戏...",
      "elapsed_ms": 1523,
      "created_at": "2026-06-02T14:30:00Z"
    },
    {
      "id": 99,
      "session_id": 5,
      "user_id": 12345,
      "role": "user",
      "content": "推荐一款游戏",
      "elapsed_ms": null,
      "created_at": "2026-06-02T14:29:55Z"
    }
  ]
}
```

**响应字段说明：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `messages` | `Array` | 消息数组（最多 10 条），按时间倒序排列 |
| `messages[].id` | `i64` | 消息唯一 ID |
| `messages[].session_id` | `i64` | 所属会话 ID |
| `messages[].user_id` | `i64` | 用户 ID |
| `messages[].role` | `String` | 角色：`user` / `assistant` / `tool` / `system` |
| `messages[].content` | `Option<String>` | 消息文本内容 |
| `messages[].elapsed_ms` | `Option<i32>` | assistant 消息的处理耗时（毫秒），user 消息为 null |
| `messages[].created_at` | `DateTime` | 消息创建时间（UTC） |

**错误响应：**

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（`user_id` ≤ 0、`date` 为空或格式错误） |
| `400` | MD5 签名校验失败 |
| `500` | 数据库查询失败 |

---

## 内部 API

> **占位**：内部 API 面向管理中心后台及用户中心，需要 JWT 鉴权（管理员或用户 Token），包括但不限于以下模块：

- **管理员认证** — 登录、登出、Token 刷新
- **管理员管理** — 增删改查、角色权限控制
- **仪表盘** — 数据概览统计
- **推荐游戏管理** — 完整 CRUD
- **插件/函数/工具/技能管理** — 004 Agent Runtime 能力注册
- **分类/标签管理** — 内容组织
- **运行时管理** — Agent 实例池、Workflow 执行器
- **审计日志** — 管理员操作审计 & 运行时审计
- **登录记录** — 登录历史查询
- **用户管理** — 用户增删改查、会话管理
- **聊天管理** — 会话查询、对话历史
- **全局配置** — 系统参数动态配置
- **游戏管理** — 后台游戏信息维护

> 详细文档将随各模块的规范完善后补充。
