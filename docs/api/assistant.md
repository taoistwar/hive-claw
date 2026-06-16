# AI 助手对话

```
POST /api/assistant
```

向 AI 助手发送消息并获取回复。

## 鉴权

MD5 签名鉴权。

请求需要附带 `sign` 查询参数，其值为：

```
MD5(ASSISTANT_SECRET + "/api/assistant" + "?body=" + 请求体 JSON 字符串)
```

> 若环境变量 `ASSISTANT_SECRET` 为空，则跳过签名校验。

## 请求头

| Header | 值 |
|--------|-----|
| `Content-Type` | `application/json; charset=UTF-8` |

## 请求体

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

## 限流策略

- VIP 用户：默认 **50 次/天**
- 普通用户：默认 **5 次/天**
- 可通过后台 `global_config` 的 `vip_ask_times` / `normal_ask_times` 动态配置
- 同一用户同时只能有一个活跃会话（SSE 并发守卫）

## 示例请求

```bash
BODY='{"user_id":12345,"message":"你好","channel":"app","client_type":"android","client_version":"1.0.0"}'
SIGN=$(echo -n "${ASSISTANT_SECRET}/api/assistant?body=${BODY}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/assistant?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
```

## 成功响应 `200 OK`

响应体为 `ChatMessageUser` 的 JSON 序列化，字段如下：

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
    "type": "goPay",
    "info": {
      "effective_end_time": "",
      "membership_category": "",
      "level_name": "",
      "total_coins": 900000.0,
      "expire_coins_7d": 5000.0
    }
  }],
  "created_at": "2026-06-02T14:30:00Z"
}
```

> **注意：** `extensions` 始终为 JSON 数组（`ExtensionContent` 经 `flatten_extension` 合并后序列化）。
> 无扩展数据时 `extensions` 为 `null` 或省略。

### 响应字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `i64` | 消息唯一标识符 |
| `session_id` | `i64` | 会话 ID |
| `user_id` | `i64` | 用户 ID |
| `role` | `String` | 角色：固定为 `"assistant"` |
| `content` | `Option<String>` | AI 助手的完整回复文本，可能为 `null` |
| `elapsed_ms` | `Option<i32>` | LLM 处理耗时（毫秒），可能为 `null` |
| `extensions` | `Option<Value>` | 扩展对象数组。由 `ExtensionContent` 经 `flatten_extension` 扁平化处理；`data` 字段合并到顶层 |
| `created_at` | `String` | 创建时间（ISO 8601） |

### `extensions` 数组元素结构

每个元素对应一个 `ExtensionContent`，其 `data` 中的字段被合并（flatten）到顶层，与 `content_type` 同级：

| 字段 | 类型 | 说明 |
|------|------|------|
| `content_type` | `String` | 内容类型：`card` / `image` / `suggestion` / `link` / `button` / `table` / `chart` / `object_ref` |
| `type` | `String` | 卡片类型（`content_type` 为 `card` 时，来自 `data.type`） |
| `info` | `Object` | 负载数据（来自 `data.info`，结构随 `type` 不同而变化） |

### 卡片类型说明

| type | 触发条件 | 说明 |
|------|----------|------|
| `subscribe` | 无有效会员 | 会员订购卡片 |
| `upgrade` | 建议升级 | 会员升级卡片 |
| `repay` | 会员即将到期（≤ 7 天） | 会员订购/续费卡片 |
| `sufficient` | 用户已有充足权益 | 可轻提示当前权益充足，不强推 |
| `game` | 游戏推荐 | 游戏推荐卡片 |

### `info` 对象字段

以下为 `goPay`/`subscribe`/`upgrade`/`repay`/`sufficient` 卡片的 `info` 字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `effective_end_time` | `String` | 会员有效期截止时间（无会员时为空字符串） |
| `membership_category` | `String` | 会员类别：SUBSCRIPTION-订阅型，E_TIME-一次性 |
| `membership_level` | `String` | 等级编码 |
| `total_coins` | `f64` | 总金币数 |
| `expire_coins_7d` | `f64` | 7 天内即将过期的金币数 |

`game` 卡片时，`info` 对象的内容和接口 [获取热门推荐游戏](recommended-games-top.md) 的 `data` 数组元素一致。

## 错误响应

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（`user_id` ≤ 0、`message` 为空、签名错误、外部用户不存在、限流超限等） |
| `429` | 同一用户已有活跃会话 |
| `500` | 内部错误（外部 DB 不可用、orchestrator 异常等） |

## 错误回滚

当 LLM 执行过程中出现错误时，系统会自动回滚当日的配额计数器，确保不会因失败而消耗用户配额。
