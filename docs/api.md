# API 文档

## 对外 API

以下 API 无需 JWT 鉴权，对所有外部调用方开放。

---

### 1. 获取热门推荐游戏

```
POST /api/recommended-games/top?sign={md5}
```

按策略过滤后返回热门推荐游戏列表，按标签分组限量返回。

**鉴权：** MD5 签名鉴权。

**鉴权方式：**

请求 URL 中需要附带 `sign` 查询参数，其值为：

```
MD5(ASSISTANT_SECRET + "/api/recommended-games/top" + "?body=" + 请求体 JSON 字符串)
```

> 若环境变量 `ASSISTANT_SECRET` 为空，则跳过签名校验。

**请求头：**

| Header | 值 |
|--------|-----|
| `Content-Type` | `application/json; charset=UTF-8` |

**请求体：**

```json
{
  "user_id": "12345",
  "channel": "app",
  "client_type": "android",
  "client_version": "1.0.0"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `user_id` | `String` | 是 | 用户 ID，不能为空 |
| `channel` | `String` | 是 | 用户当前渠道标识（如 `app`、`web`），不能为空 |
| `client_type` | `String` | 是 | 客户端类型（如 `android`、`iphone`、`pc`），不能为空 |
| `client_version` | `String` | 是 | 客户端版本号，不能为空 |

**策略过滤规则：**

每条推荐游戏可配置多条策略（`recommended_games_strategy` 表）。策略包含三个要素：

| 要素 | 说明 |
|------|------|
| `strategy` | `INCLUDE` — 包含 / `EXCLUDE` — 排除 |
| `channel` | 渠道列表（JSON 数组），`"*"` 表示全部渠道 |
| `client_type` | 客户端类型列表（JSON 数组），`"*"` 表示全部客户端 |

过滤逻辑（对每条游戏）：

1. 游戏无策略 → **不展示**
2. 存在 EXCLUDE 策略匹配当前用户 → **不展示**（EXCLUDE 优先）
3. 存在 INCLUDE 策略匹配当前用户 → **展示**
4. 无任何策略匹配 → **不展示**

策略匹配条件：用户的 `channel` 在策略的 `channel` 列表中（或列表含 `"*"`）**且** 用户的 `client_type` 在策略的 `client_type` 列表中（或列表含 `"*"`）。

**按标签限量：**

| 标签 | 返回数量 |
|------|----------|
| `运营推荐` | 4 条 |
| `新游上线` | 3 条 |
| `本周热玩` | 3 条 |

返回顺序：运营推荐 → 新游上线 → 本周热玩。

**示例请求：**

```bash
BODY='{"user_id":"12345","channel":"app","client_type":"android","client_version":"1.0.0"}'
SIGN=$(echo -n "${ASSISTANT_SECRET}/api/recommended-games/top?body=${BODY}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/recommended-games/top?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
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
      "tag": "运营推荐",
      "game_category": [{"name": "角色扮演", "type": "RPG"}],
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
| `tag` | `Option<String>` | 标签：运营推荐 / 新游上线 / 本周热玩 |
| `game_category` | `Option<Vec<Object>>` | 游戏分类（JSON 数组），每项含 `name` 和 `type` 字段 |
| `game_image` | `Option<String>` | 游戏封面图片 URL |
| `game_id` | `String` | 关联游戏 ID |
| `game_name` | `String` | 关联游戏名称 |

**错误响应：**

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（必填参数缺失或为空字符中、MD5 签名校验失败） |
| `500` | 数据库查询失败 |

---

### 2. 执行推荐游戏

```
POST /api/recommended-games/execute?sign={md5}
```

点击推荐游戏后，自动记录两条聊天消息（用户消息 + assistant 游戏卡片）。

**鉴权：** MD5 签名鉴权。

**鉴权方式：**

请求 URL 中需要附带 `sign` 查询参数，其值为：

```
MD5(ASSISTANT_SECRET + "/api/recommended-games/execute" + "?body=" + 请求体 JSON 字符串)
```

> 若环境变量 `ASSISTANT_SECRET` 为空，则跳过签名校验。

**请求头：**

| Header | 值 |
|--------|-----|
| `Content-Type` | `application/json; charset=UTF-8` |

**请求体：**

```json
{
  "user_id": "12345",
  "game_id": "game_001",
  "channel": "app",
  "client_type": "android",
  "client_version": "1.0.0"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `user_id` | `String` | 是 | 用户 ID，不能为空 |
| `game_id` | `String` | 是 | 推荐游戏的 game_id，不能为空 |
| `channel` | `String` | 否 | 渠道标识（如 `app`、`web`） |
| `client_type` | `String` | 否 | 客户端类型（如 `android`、`iphone`、`web`） |
| `client_version` | `String` | 否 | 客户端版本号 |

**处理流程：**

1. 通过 `game_id` 查询推荐游戏记录
2. 获取或创建用户会话（`chat_sessions_user`）
3. 记录一条 `role=user` 消息，内容为游戏的 `reply` 字段
4. 记录一条 `role=assistant` 消息，内容为空，`extensions` 包含一个 `game` 类型的 card

**示例请求：**

```bash
BODY='{"user_id":"12345","game_id":"game_001","channel":"app","client_type":"android","client_version":"1.0.0"}'
SIGN=$(echo -n "${ASSISTANT_SECRET}/api/recommended-games/execute?body=${BODY}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/recommended-games/execute?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
```

**成功响应 `200 OK`：**

```json
{
  "code": 0,
  "data": {
    "id": 200,
    "session_id": 5,
    "user_id": 12345,
    "role": "assistant",
    "content": "",
    "elapsed_ms": null,
    "extensions": [
      {
        "content_type": "card",
        "payload": {
          "type": "game",
          "info": {
            "game_id": "game_001",
            "game_name": "游戏名称",
            "name": "推荐标题",
            "reply": "推荐回复内容",
            "reason": "推荐理由",
            "tag": "运营推荐",
            "game_category": [{"name": "角色扮演", "type": "RPG"}],
            "game_image": "https://example.com/image.png"
          }
        }
      }
    ],
    "created_at": "2026-06-10T14:30:00Z"
  },
  "message": "ok"
}
```

**`extensions[].payload.info` 字段说明：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `game_id` | `String` | 游戏 ID |
| `game_name` | `String` | 游戏名称 |
| `name` | `String` | 推荐标题 |
| `reply` | `String` | 推荐回复内容 |
| `reason` | `Option<String>` | 推荐理由 |
| `tag` | `Option<String>` | 标签：运营推荐 / 新游上线 / 本周热玩 |
| `game_category` | `Option<Value>` | 游戏分类（JSON） |
| `game_image` | `Option<String>` | 游戏封面图片 URL |

**错误响应：**

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（`user_id` 或 `game_id` 为空、MD5 签名校验失败） |
| `404` | `game_id` 对应的推荐游戏不存在 |
| `500` | 数据库查询或写入失败 |

---

### 3. AI 助手对话

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
  "client_type": "android",
  "client_version": "1.0.0",
  "new_session": false
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `user_id` | `i64` | 是 | 用户 ID，必须大于 0 |
| `message` | `String` | 是 | 用户消息内容，不能为空或全空白 |
| `channel` | `String` | 是 | 渠道标识（如 `app`、`web`、`api`） |
| `client_type` | `String` | 是 | 客户端平台（`android`、`iphone`、`ipad`、`web`） |
| `client_version` | `String` | 是 | 客户端版本号 |
| `new_session` | `bool` | 否 | 是否创建新会话，默认 `false`。`true` 时强制创建新 session；`false`/省略时复用最新 session |

**限流策略：**

- VIP 用户：默认 **50 次/天**
- 普通用户：默认 **5 次/天**
- 可通过后台 `global_config` 的 `vip_ask_times` / `normal_ask_times` 动态配置
- 同一用户同时只能有一个活跃会话（SSE 并发守卫）

**示例请求：**

```bash
BODY='{"user_id":12345,"message":"你好","channel":"app","client_type":"android","client_version":"1.0.0"}'
SIGN=$(echo -n "${ASSISTANT_SECRET}/api/assistant?body=${BODY}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/assistant?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
```

**成功响应 `200 OK`：**

```json
{
  "id": 100,
  "session_id": 5,
  "user_id": 12345,
  "role": "assistant",
  "content": "您好！以下是为您推荐的游戏...",
  "elapsed_ms": 1523,
  "extensions": [{
    "content_type": "card",
    "payload": {
      "type": "goPay",
      "info": {
        "effective_end_time": "",
        "membership_category": "",
        "level_name": "",
        "total_coins": 900000.0,
        "expire_coins_7d": 5000.0
      }
    }
  }],
  "created_at": "2026-06-02T14:30:00Z"
}
```

> **注意：** 当扩展对象只有 **1 条**时，API 返回 `extension`（单数，对象格式）；
> 当有 **2 条及以上**时，返回 `extensions`（复数，数组格式）；
> 无扩展数据时，两个字段均省略。

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `i64` | 唯一标识符 |
| `session_id` | `i64` | 会话 ID |
| `user_id` | `i64` | 用户 ID |
| `role` | `String` | 角色：`assistant` |
| `content` | `String` | AI 助手的完整回复文本 |
| `elapsed_ms` | `Option<u64>` | LLM 处理耗时（毫秒） |
| `extensions` | `Option<Vec<Value>>` | 扩展对象数组，仅当扩展数量 ≥ 2 时出现 |
| `created_at` | `String` | 创建时间 |

**`extensions`对象 structure：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `content_type` | `String` | 内容类型：`card` / `image` / `suggestion` / `link` / `button` / `table` / `chart` / `object_ref` |
| `payload.type` | `String` | 卡片类型（当 `content_type` 为 `card` 时） |
| `payload.info` | `Object` | 负载数据（当 `type` 为 `goPay`/`subscribe`/`upgrade`/`sufficient`/`repay`/`game` 时） |


**卡片类型说明：**

| type | 触发条件 | 说明 |
|------|----------|------|
| `goPay` | 金币不足（余额 < 500） | 充值卡片 |
| `subscribe` | 无有效会员 | 会员订购卡片 |
| `upgrade` | 建议升级 | 会员升级卡片 |
| `repay` | 会员即将到期（≤ 7 天） | 会员订购/续费卡片 |
| `sufficient` | 用户已有充足权益 | 可轻提示当前权益充足，不强推 |
| `game` | 游戏推荐 | 游戏推荐卡片 |


**`info` 对象字段：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `effective_end_time` | `String` | 会员有效期截止时间（无会员时为空字符串） |
| `membership_category` | `String` | 会员类别：SUBSCRIPTION-订阅型，E_TIME-一次性 |
| `membership_level` | `String` | 等级编码 |
| `total_coins` | `f64` | 总金币数 |
| `expire_coins_7d` | `f64` | 7 天内即将过期的金币数 |

`game`卡片时，`info`对象的内容和接口`获取热门推荐游戏`的`data`数组元素一致。

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

### 4. 获取用户历史消息

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
      "extensions": [{
        "content_type": "card",
        "payload": {
          "type": "goPay",
          "info": {
            "effective_end_time": "",
            "membership_category": "",
            "level_name": "",
            "total_coins": 900000.0,
            "expire_coins_7d": 5000.0
          }
        }
      }],
      "created_at": "2026-06-02T14:30:00Z"
    },
    {
      "id": 99,
      "session_id": 5,
      "user_id": 12345,
      "role": "user",
      "content": "推荐一款游戏",
      "elapsed_ms": null,
      "extension": null,
      "extensions": null,
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
| `messages[].extensions` | `Option<Vec<Value>>` | 扩展对象数组（≥ 2 条时出现），结构与 `/api/assistant` 一致 |
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
