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
  "content": "您好！以下是为您查询的会员信息...",
  "elapsed_ms": 1523,
  "extensions": [{
    "content_type": "card",
    "payload": {
      "type": "sufficient",
      "info": {
        "disk_end_time": 1735689600,
        "disk_total_size": 1099511627776,
        "total_coins": 900000.0,
        "expire_coins_7d": 5000.0
      },
      "membership": [
        {
          "membership_level": "vip",
          "level_name": "VIP",
          "membership_category": "SUBSCRIPTION",
          "membership_category_name": "订阅型",
          "effective_start_time": "2025-01-01 00:00:00",
          "effective_end_time": "2026-12-31 23:59:59",
          "product_title": "VIP月度订阅",
          "subscription_id": "sub_abc123",
          "subscription_status": "active",
          "subscription_status_name": "生效中",
          "next_billing_time": "2026-07-01 00:00:00",
          "auto_renew": true,
          "payment_method": "alipay",
          "subscription_start_time": "2025-01-01 00:00:00",
          "subscription_end_time": "2026-12-31 23:59:59"
        }
      ],
      "duration_card": [
        {
          "card_asset_id": "card_001",
          "remain_duration": 86400,
          "computer_biz_type": "game",
          "expire_time": 1735689600,
          "card_type": "time",
          "card_type_name": "时长卡",
          "order_id": "order_001",
          "consume_label": "游戏时长",
          "extra": null,
          "create_time": "2025-06-01 12:00:00",
          "fps": "60",
          "gpu": "4070"
        }
      ]
    }
  }],
  "created_at": "2026-06-02T14:30:00Z"
}
```

> **注意：** `extensions` 始终为 JSON 数组。`content_type` 为 `card` 时，`payload` 是一个嵌套对象，
> 包含 `type`、`info`、`membership`、`duration_card` 等字段。
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
| `extensions` | `Option<Value>` | 扩展对象数组。每个元素包含 `content_type` 和 `payload` 字段 |
| `created_at` | `String` | 创建时间（ISO 8601） |

### `extensions` 数组元素结构

每个元素对应一个 `ExtensionContent`：

| 字段 | 类型 | 说明 |
|------|------|------|
| `content_type` | `String` | 内容类型：`card` / `image` / `suggestion` / `link` / `button` / `table` / `chart` / `object_ref` |
| `payload` | `Object` | 负载数据，结构随 `content_type` 不同而变化 |

当 `content_type` 为 `card` 时，`payload` 结构如下：

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | `String` | 卡片类型：`subscribe` / `repay` / `upgrade` / `sufficient` / `game` |
| `info` | `Object` | 余额与磁盘信息 |
| `membership` | `Array<Object>` | 会员订阅列表（仅在会员相关卡片中出现） |
| `duration_card` | `Array<Object>` | 时长卡列表（仅在会员相关卡片中出现） |

### 卡片类型说明

| type | 触发条件 | 说明 |
|------|----------|------|
| `subscribe` | 无有效会员 | 引导首次开通会员 |
| `repay` | 会员即将到期（≤ 7 天） | 引导续费 |
| `upgrade` | 已有会员但非最高等级 | 引导升级到更高等级 |
| `sufficient` | 已有充足权益 | 权益充足，不强推付费 |
| `game` | 游戏推荐 | 游戏推荐卡片 |

### `info` 对象字段

以下为 `subscribe`/`repay`/`upgrade`/`sufficient` 卡片的 `info` 字段（来自用户余额查询结果）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `disk_end_time` | `u64` | 网盘截止时间（Unix 时间戳），0 表示无 |
| `disk_total_size` | `f64` | 网盘总大小（字节） |
| `total_coins` | `f64` | 总金币数 |
| `expire_coins_7d` | `f64` | 7 天内即将过期的金币数 |

### `membership` 数组元素字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `membership_level` | `String` | 会员等级编码 |
| `level_name` | `String` | 等级显示名称 |
| `membership_category` | `String` | 会员类别：`SUBSCRIPTION`（订阅型）/ `E_TIME`（一次性） |
| `membership_category_name` | `String` | 类别显示名称 |
| `effective_start_time` | `Option<String>` | 生效开始时间 |
| `effective_end_time` | `Option<String>` | 生效截止时间 |
| `product_title` | `Option<String>` | 商品标题 |
| `subscription_id` | `Option<String>` | 订阅 ID |
| `subscription_status` | `Option<String>` | 订阅状态 |
| `subscription_status_name` | `Option<String>` | 订阅状态显示名称 |
| `next_billing_time` | `Option<String>` | 下次扣费时间 |
| `auto_renew` | `Option<bool>` | 是否自动续费 |
| `payment_method` | `Option<String>` | 支付方式 |
| `subscription_start_time` | `Option<String>` | 订阅开始时间 |
| `subscription_end_time` | `Option<String>` | 订阅结束时间 |

### `duration_card` 数组元素字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `card_asset_id` | `Option<String>` | 卡资产 ID |
| `remain_duration` | `Option<i64>` | 剩余时长（秒） |
| `computer_biz_type` | `Option<String>` | 计算业务类型 |
| `expire_time` | `Option<i64>` | 过期时间（Unix 时间戳） |
| `card_type` | `Option<String>` | 卡类型编码 |
| `card_type_name` | `Option<String>` | 卡类型显示名称 |
| `order_id` | `Option<String>` | 订单 ID |
| `consume_label` | `Option<String>` | 消费标签 |
| `extra` | `Option<Value>` | 额外信息（JSON） |
| `create_time` | `Option<String>` | 创建时间 |
| `fps` | `Option<String>` | 帧率（从 `product_mirror.fps` 提取，仅非空 JSON 时有值） |
| `gpu` | `Option<String>` | GPU 型号（从 `product_mirror.gpu` 提取，仅非空 JSON 时有值） |

`game` 卡片时，`payload` 结构参见 [获取热门推荐游戏](recommended-games-top.md)。

## 错误响应

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（`user_id` ≤ 0、`message` 为空、签名错误、外部用户不存在、限流超限等） |
| `429` | 同一用户已有活跃会话 |
| `500` | 内部错误（外部 DB 不可用、orchestrator 异常等） |

## 错误回滚

当 LLM 执行过程中出现错误时，系统会自动回滚当日的配额计数器，确保不会因失败而消耗用户配额。
