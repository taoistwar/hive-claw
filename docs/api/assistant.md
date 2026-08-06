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
- 同一用户同时只能有一个活跃 Assistant 请求（按用户并发守卫）

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
        "disk_end_time": 1735689600000,
        "disk_end_date": "2025-01-01",
        "disk_total_size": 50,
        "disk_total_size_text": "50GB",
        "total_coins": 900000.0,
        "expire_coins_7d": 5000.0,
        "disk_status": "NORMAL",
        "disk_status_text": "生效中"
      },
      "membership": [
        {
          "auto_renew": false,
          "level_name": "史诗会员",
          "product_title": "连续包月",
          "payment_method": "WECHAT_PAY_CONTRACT_ENTRUST_WEB",
          "subscription_id": 357238,
          "membership_level": "EPIC",
          "next_billing_time": "2026-07-04 22:17:14",
          "effective_end_time": "2026-07-04 23:59:59",
          "membership_category": "SUBSCRIPTION",
          "subscription_status": "REVOKE",
          "effective_start_time": "2026-06-05 00:00:00",
          "subscription_end_time": "2026-06-04 07:41:55",
          "subscription_start_time": "2026-06-04 22:17:14",
          "membership_category_name": "订阅型",
          "subscription_status_name": "已解约"
        }
      ],
      "duration_card": [
        {
            "order_id": -1,
            "create_time": "2026-06-16 16:00:15 UTC",
            "expire_time": 1781884815303,
            "card_asset_id": 30123507,
            "consume_label": {
                "weight": 99,
                "channelList": [
                    "ALL"
                ],
                "gameLabelList": [
                    "FREE_CARD",
                    "TASK_FREE_CARD"
                ],
                "clientTypeList": [
                    "ALL"
                ]
            },
            "remain_duration": 300000,
            "computer_biz_type": null,
            "game_label_list": ["手游", "PC游戏"],
            "product_title": "金卡",
            "product_duration": "3600000"
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
| `content_type` | `String` | 内容类型：`card` / `image` / `usage` / `suggestion` / `link` / `button` / `table` / `chart` / `object_ref` |
| `payload` | `Object` | 负载数据，结构随 `content_type` 不同而变化 |

当 `content_type` 为 `card` 时，`payload` 结构如下：

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | `String` | 卡片类型：`subscribe` / `repay` / `upgrade` / `sufficient` / `game` / `support` / `discount` |
| `info` | `Object` | 卡片信息（余额/游戏详情等，随 `type` 不同而变化） |
| `membership` | `Array<Object>` | 会员订阅列表（仅在会员相关卡片中出现） |
| `duration_card` | `Array<Object>` | 时长卡列表（仅在会员相关卡片中出现） |

### 卡片类型说明

| type | 触发条件 | 说明 |
|------|----------|------|
| `subscribe` | 无有效会员 | 引导首次开通会员 |
| `repay` | 会员即将到期（≤ 7 天） | 引导续费 |
| `upgrade` | 已有会员但非最高等级 | 引导升级到更高等级 |
| `sufficient` | 已有充足权益 | 权益充足，不强推付费 |
| `game` | 游戏查询或推荐 | 游戏卡片；Agent builtin 仍在使用，与已废弃的推荐游戏管理业务不是同一功能 |
| `support` | 用户请求人工客服 | 转接客服卡片，携带用户原始输入 |
| `discount` | 用户查询优惠活动 | 优惠产品卡片，展示当前渠道/端的折扣产品 |

### `info` 对象字段（subscribe / repay / upgrade / sufficient 卡片）

以下为 `subscribe`/`repay`/`upgrade`/`sufficient` 卡片的 `info` 字段（来自用户余额查询结果）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `disk_end_time` | `u64` | 网盘截止时间（Unix 毫秒时间戳），0 表示无；保留供卡片兼容使用 |
| `disk_end_date` | `String \| null` | 按北京时间格式化的网盘到期日期（`YYYY-MM-DD`），供模型和文本展示直接使用 |
| `disk_total_size` | `f64` | 网盘总容量，单位为 GB；保留供卡片兼容使用 |
| `disk_total_size_text` | `String` | 带 GB 单位的展示容量，如 `50GB`，供模型直接使用 |
| `total_coins` | `f64` | 总金币数 |
| `expire_coins_7d` | `f64` | 7 天内即将过期的金币数 |
| `disk_status` | `String` | 网盘原始状态码：`EXPIRED`（已过期）、`NORMAL`（生效中）、`NOT_ALLOCATE`（未挂载）、`RESERVE_PERIOD`（保留期） |
| `disk_status_text` | `String` | 网盘状态中文：`已过期`、`生效中`、`未挂载`或`保留期`，供模型直接使用 |

### `info` 对象字段（game 卡片）

以下为 `game` 卡片的 `info` 字段。当前 Agent builtin 可从外部游戏 DB 生成该卡片；
历史上的推荐游戏执行接口也复用了这一结构，但该接口及其 `recommended_games` 业务
已经废弃，新集成不得依赖该遗留入口。

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `String` | 游戏逻辑 ID；遗留推荐游戏接口中曾对应 `recommended_games.game_id` |
| `name` | `String` | 游戏名称；遗留推荐游戏接口中曾对应 `recommended_games.game_name` |
| `channel` | `String` | 请求时的渠道标识 |
| `client_type` | `String` | 请求时的客户端类型 |
| `reason` | `Option<String>` | 推荐理由 |
| `game_tags` | `Option<Value>` | 游戏标签数组，元素为 `{"name": "标签名", "type": 1}` |
| `cover_image` | `Option<String>` | 游戏封面图片 URL |
| `computer_id` | `Option<i64>` | 外部游戏表关联的 computer_id |
| `platform_name` | `Option<String>` | 平台名称（如 Steam） |
| `game_icon` | `Option<String>` | 游戏图标 URL |

**game 卡片示例：**

```json
{
  "content_type": "card",
  "payload": {
    "type": "game",
    "info": {
      "id": "208",
      "name": "最终幻想7：重制版",
      "channel": "haimayun",
      "client_type": "ANDROID",
      "reason": "因跌宕起伏的剧情与充满魅力的角色...",
      "game_tags": [{"name": "角色扮演", "type": 1}],
      "cover_image": "https://example.com/cover.jpg",
      "computer_id": 10269,
      "platform_name": "Steam",
      "game_icon": "https://example.com/icon.png"
    }
  }
}
```

### `info` 对象字段（support 卡片）

`support` 卡片无 `info`/`membership`/`duration_card` 字段，仅包含 `type`：

```json
{
  "content_type": "card",
  "payload": {
    "type": "support"
  }
}
```

触发条件：用户明确表达需要人工客服时，由 Agent 调用 `support_card` builtin 生成此卡片并立即结束 agent loop。

### `payload` 对象字段（discount 卡片）

`discount` 卡片包含 `discount` 字段（注意不是 `info`），直接来自 `cc_config` 表 `AIDiscountedProducts` 配置（经 `client_type` / `channel` / `__DEFAULT__` 回退解析）：

**discount 卡片示例：**

```json
{
  "content_type": "card",
  "payload": {
    "type": "discount",
    "discount": {
      "link": "https://h5.haimacloud.com/activity/huyalive?style=1",
      "bgimg": "https://pc-cos.haimacloud.com/game/1498/cover/11d971e6.webp",
      "buttonImg": "https://pc-cos.haimacloud.com/logicGame/ee6d1de2.png",
      "titleDesc": "国庆特惠",
      "buttonDesc": "立即跳转",
      "titleColor": "#DB4040",
      "buttonColor": "#567CCC",
      "description": "无限暖暖国庆大促",
      "descriptionColor": "#40820E"
    }
  }
}
```

**`discount` 对象字段：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `link` | `String` | 活动跳转链接 |
| `bgimg` | `String` | 背景图片 URL |
| `titleDesc` | `String` | 标题文案 |
| `titleColor` | `String` | 标题颜色（如 `#DB4040`） |
| `buttonImg` | `String` | 按钮图片 URL |
| `buttonDesc` | `String` | 按钮文案 |
| `buttonColor` | `String` | 按钮颜色（如 `#567CCC`） |
| `description` | `String` | 描述文案 |
| `descriptionColor` | `String` | 描述颜色（如 `#40820E`） |

> 以上字段为白名单过滤后的结果，`AIDiscountedProducts` 中的其他字段不会传递给前端。
> 触发条件：用户询问优惠/折扣/促销活动时，Agent 调用 `query_balance(category="discount")` 获取当前渠道和端的优惠产品配置。

### `content_type` 为 `usage` 时的结构

当用户当日剩余可用次数达到提醒阈值时，响应中的 `extensions` 会追加一个 `usage` 类型的扩展，用于提示用户剩余配额。

触发条件：
- `remain_ask_time > 0`（已配置提醒阈值，默认 **2**）
- 剩余次数 `remaining > 0`（尚未超过限额）
- `remaining <= remain_ask_time`（剩余次数不超过阈值）

**`usage` 扩展示例：**

```json
{
  "content_type": "usage",
  "payload": {
    "used_times": 8,
    "total_times": 10,
    "membership_max_times": 50,
    "remain_ask_time": 2
  }
}
```

**`usage` payload 字段：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `used_times` | `i64` | 当日已使用次数 |
| `total_times` | `i64` | 当日总可用次数（VIP / 普通用户上限，由 `cc_config` 表动态配置） |
| `membership_max_times` | `i64` | 会员（VIP）每日最大可用次数，用于前端展示升级引导 |
| `remain_ask_time` | `i64` | 剩余提醒阈值，当 `remaining <=` 该值时触发 usage 扩展；`0` 表示关闭提醒 |

> 该扩展由 `chat_assistant_handler` 在响应返回前根据限流计数结果动态注入，与 Agent 执行过程无关。
> 可通过后台 `cc_config` 表 `AIassistantChatLimitConfig` 配置项中的 `remain_ask_time` 调整提醒阈值（设为 `0` 关闭提醒）。

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
| `next_price` | `Option<i32>` | 下次扣款费用（单位：分） |

### `duration_card` 数组元素字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `card_asset_id` | `Option<i64>` | 卡资产 ID |
| `remain_duration` | `Option<i64>` | 剩余时长（秒） |
| `computer_biz_type` | `Option<String>` | 计算业务类型 |
| `expire_time` | `Option<i64>` | 过期时间（Unix 时间戳） |
| `order_id` | `Option<i64>` | 订单 ID |
| `consume_label` | `Option<Value>` | 消费标签（JSON 对象，含 `weight`、`channelList`、`gameLabelList`、`clientTypeList`） |
| `create_time` | `Option<String>` | 创建时间 |
| `game_label_list` | `Option<Value>` | 游戏标签列表（JSON 数组，码值经 `cc_label` 表解析为中文，如 `["手游","PC游戏","90系云电脑120帧"]`） |
| `product_title` | `Option<String>` | 商品名称（如"30小时畅玩"、"60小时畅玩"） |
| `product_duration` | `Option<String>` | 商品时长（毫秒字符串） |


## 配额查询

```
GET /api/quota?sign={md5}&user_id={user_id}
```

查询用户当日配额使用情况，**只读操作不消耗配额**。

### 鉴权

与 [`POST /api/assistant`](#鉴权) 一致，MD5 签名校验。签名计算方式：

```
MD5(ASSISTANT_SECRET + "/api/quota" + "?body=" + "user_id=12345")
```

> 注意：`body` 参数为 `user_id=<value>` 字符串。

### 请求参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `sign` | `String` | 是* | MD5 签名（`ASSISTANT_SECRET` 为空时跳过） |
| `user_id` | `i64` | 是 | 用户 ID，必须大于 0 |

### 成功响应 `200 OK`

```json
{
  "code": 200,
  "data": {
    "used_times": 8,
    "total_times": 10,
    "membership_max_times": 50,
    "remain_ask_time": 2
  }
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `used_times` | `i64` | 当日已使用次数 |
| `total_times` | `i64` | 当日总可用次数（VIP 取 `vip_ask_times`，普通取 `normal_ask_times`） |
| `membership_max_times` | `i64` | 会员（VIP）每日最大可用次数，用于前端展示升级引导 |
| `remain_ask_time` | `i64` | 剩余提醒阈值，`0` 表示关闭提醒 |

### 示例请求

```bash
USER_ID=12345
SIGN=$(echo -n "${ASSISTANT_SECRET}/api/quota?body=user_id=${USER_ID}" | md5sum | awk '{print $1}')

curl "http://localhost:3300/api/quota?sign=${SIGN}&user_id=${USER_ID}"
```

## 错误响应

所有错误响应遵循统一格式：

```json
{
  "code": 4290,
  "message": "Daily limit reached (10/10)"
}
```

| `code` | HTTP 状态码 | `message` | 说明 |
|--------|-------------|-----------|------|
| `0` | `200` | `success` | 请求成功 |
| `4290` | `429` | `Daily limit reached (50/50)` | 日访问次数超限（括号内为 `total_times/total_times`） |
| `4291` | `429` | `并发会话过多，请关闭其它对话窗口后重试` | 同一用户已有活跃 Assistant 请求 |
| `4009` | `400` | `内容安全警告：输入的文本数据可能包含不适当的内容！` | 敏感词过滤拦截 |
| `4000` | `400` | 动态消息（如 `Invalid signature`、`User not found`、`user_id must be positive`、`message must not be empty` 等） | 参数校验/鉴权失败 |
| `6004` | `408` | Hook 超时的普通文本消息 | 阻塞模式 Hook 执行超时，Agent 请求已中止 |
| `6005` | `500` | Hook 失败的普通文本消息 | 阻塞模式 Hook 执行失败，Agent 请求已中止 |
| `5000` | `500` | 动态消息（如 `Redis unavailable`、`Assistant service unavailable`、orchestrator 异常等） | 内部错误 |

Hook 运行时错误由本接口直接返回同步 JSON，不提供管理聊天或 admin SSE 错误契约。发生阻塞 Hook 错误时：

- 当日配额恰好回滚一次；若 Redis 回滚失败，只记录 tracing，不覆盖原错误。
- `on_agent_error` 仍以 tracing-only 方式执行。
- 跳过 `after_agent_end`，且不持久化 assistant 占位消息。
- 内部错误码或消息无效时降级为 5000/HTTP 500。

### 错误响应示例

**超出日限额：**

```json
{
  "code": 4290,
  "message": "Daily limit reached (10/10)"
}
```

**并发请求冲突：**

```json
{
  "code": 4291,
  "message": "并发会话过多，请关闭其它对话窗口后重试"
}
```

**阻塞 Hook 超时：**

```json
{
  "code": 6004,
  "message": "Hook「slow-hook」阻塞模式执行超时"
}
```

**阻塞 Hook 执行失败：**

```json
{
  "code": 6005,
  "message": "Hook「guard-hook」阻塞模式执行失败"
}
```

**签名错误：**

```json
{
  "code": 4000,
  "message": "Invalid signature"
}
```

**用户不存在：**

```json
{
  "code": 4000,
  "message": "User not found"
}
```

**敏感词拦截：**

```json
{
  "code": 4009,
  "message": "内容安全警告：输入的文本数据可能包含不适当的内容！"
}
```

## 错误回滚

当 LLM 执行过程中出现错误时，系统会自动回滚当日的配额计数器，确保不会因失败而消耗用户配额。
