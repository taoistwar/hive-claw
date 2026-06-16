# 获取热门推荐游戏

```
POST /api/recommended-games/top?sign={md5}
```

按策略过滤后返回热门推荐游戏列表，按标签分组限量返回。

## 鉴权

MD5 签名鉴权。

请求 URL 中需要附带 `sign` 查询参数，其值为：

```
MD5(ASSISTANT_SECRET + "/api/recommended-games/top" + "?body=" + 请求体 JSON 字符串)
```

> 若环境变量 `ASSISTANT_SECRET` 为空，则跳过签名校验。

## 请求头

| Header | 值 |
|--------|-----|
| `Content-Type` | `application/json; charset=UTF-8` |

## 请求体

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

## 策略过滤规则

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

## 按标签限量

| 标签 | 返回数量 |
|------|----------|
| `运营推荐` | 4 条 |
| `新游上线` | 3 条 |
| `本周热玩` | 3 条 |

返回顺序：运营推荐 → 新游上线 → 本周热玩。

## 示例请求

```bash
BODY='{"user_id":"12345","channel":"app","client_type":"android","client_version":"1.0.0"}'
SIGN=$(echo -n "${ASSISTANT_SECRET}/api/recommended-games/top?body=${BODY}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/recommended-games/top?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
```

## 成功响应 `200 OK`

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

### 字段说明

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

## 错误响应

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（必填参数缺失或为空字符串、MD5 签名校验失败） |
| `500` | 数据库查询失败 |
