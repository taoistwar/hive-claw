# 外部数据库 — 游戏

以下 SQL 查询对外部数据库中游戏相关表。所有查询均为只读。

查询实现位于：
- [services/game_service.rs](../../crates/hiveweb/src/services/game_service.rs)
- [api/game.rs](../../crates/hiveweb/src/api/game.rs)

---

## cc_logic_game — 逻辑游戏主表

### 全量游戏列表（无过滤，仅 id + name）

```sql
SELECT id, name FROM cc_logic_game ORDER BY id
```

- **调用方：** `api/game.rs` `get_external_games()` handler
- **返回：** `Vec<(id: i64, name: String)>` → `Vec<ExternalGameOption>`
- **Redis 缓存：** `external_games:list` — TTL 30min

### 按 cc_game.id 查询游戏信息

```sql
SELECT t1.id, t1.name, COALESCE(t1.alias, '') AS alias
FROM cc_logic_game t1
LEFT JOIN cc_game t2 ON t1.id = t2.logic_game_id
WHERE t2.id = ?
```

- **调用方：** `get_external_game_by_id()`
- **参数：** `cc_game.id`（非 `cc_logic_game.id`）
- **返回：** `Option<(id: i64, name: String, alias: String)>`
- **Redis 缓存：** `game_info:{game_id}` — TTL 15min

### 按 channel + client_type 过滤游戏列表

```sql
SELECT DISTINCT ga.id, g.name, COALESCE(g.alias, '') AS alias
FROM cc_logic_game g
INNER JOIN cc_logic_game_wide w ON w.logic_game_id = g.id
INNER JOIN cc_game ga ON ga.logic_game_id = g.id
INNER JOIN cc_logic_game_version v ON w.version = v.version
LEFT JOIN cc_logic_game_exclude e
    ON w.logic_game_id = e.logic_game_id
    AND e.client_type = w.client_type
    AND e.channel = ?
LEFT JOIN cc_logic_game_blacklist bl ON w.logic_game_id = bl.logic_game_id
WHERE w.client_type = ?
    AND w.channel_game_tag = COALESCE(
        (SELECT pc.game_tag FROM cc_promotion_channel pc WHERE pc.prom_channel = ? LIMIT 1),
        'UNKNOWN'
    )
    AND e.id IS NULL
    AND bl.id IS NULL
ORDER BY ga.id
```

- **调用方：** `list_external_games()`
- **参数：** `channel`, `client_type`, `channel`（channel 绑定 2 次）
- **返回：** `Vec<(id: u32, name: String, alias: String)>`
- **过滤规则：**
  - `cc_logic_game_wide.client_type` 匹配入参
  - 通过 `cc_promotion_channel` 将 channel 映射为 `game_tag`
  - 排除 `cc_logic_game_exclude` 中匹配的记录
  - 排除 `cc_logic_game_blacklist` 中的记录
- **Redis 缓存：** `game_list:{channel}:{client_type}` — TTL 10min

### 游戏详情（id + name + description + cover + tags）

```sql
SELECT g.id, g.name, w.description, w.cover_image, w.game_tags
FROM cc_logic_game g
LEFT JOIN cc_logic_game_wide w ON w.logic_game_id = g.id
WHERE g.id = ?
```

- **调用方：** `api/game.rs` `get_external_game_detail()` handler
- **返回：** `(id, name, description?, cover_image?, game_tags?)`
- **无 Redis 缓存**（直接查询，按需）

---

## cc_logic_game_wide — 游戏扩展信息表

### 查询游戏的 client_type

```sql
SELECT client_type FROM cc_logic_game_wide WHERE logic_game_id = ?
```

- **调用方：** `get_game_client_types()`
- **返回：** `Option<serde_json::Value>`

### 查询游戏渠道

```sql
SELECT t1.prom_channel
FROM (
    SELECT pc.prom_channel, pc.prom_platform, pc.game_tag, pc.department, pc.director
    FROM cc_logic_game_wide w
    INNER JOIN cc_promotion_channel pc
        ON pc.game_tag = w.channel_game_tag
        AND pc.status = 1
    WHERE w.logic_game_id = ?
) t1
GROUP BY t1.prom_channel
```

- **调用方：** `get_game_channels()`
- **返回：** `Vec<String>`

---

## cc_game — 游戏实例表

`cc_game` 是 `cc_logic_game` 与具体渠道/客户端的关联表。

| 字段 | 说明 |
|------|------|
| `id` | 主键（即 HiveClaw 中使用的 game_id） |
| `logic_game_id` | 关联 `cc_logic_game.id` |
| `name` | 实例名称 |

---

## cc_promotion_channel — 推广渠道表

将用户 channel 映射为 `game_tag`，用于游戏列表过滤。

| 字段 | 说明 |
|------|------|
| `prom_channel` | 推广渠道标识（与用户 `channel` 对应） |
| `game_tag` | 游戏标签（与 `cc_logic_game_wide.channel_game_tag` 关联） |
| `status` | 状态：1=启用 |
| `prom_platform` | 推广平台 |
| `department` | 部门 |
| `director` | 负责人 |

查询模式：
```sql
SELECT pc.game_tag FROM cc_promotion_channel pc WHERE pc.prom_channel = ? LIMIT 1
```

---

## cc_logic_game_exclude — 游戏排除列表

按 channel + client_type 维度排除特定游戏。

| 字段 | 说明 |
|------|------|
| `logic_game_id` | 关联 `cc_logic_game.id` |
| `channel` | 渠道 |
| `client_type` | 客户端类型 |

---

## cc_logic_game_blacklist — 游戏黑名单

全局游戏黑名单。

| 字段 | 说明 |
|------|------|
| `logic_game_id` | 关联 `cc_logic_game.id` |

---

## cc_logic_game_version — 游戏版本表

在游戏列表查询中通过 `INNER JOIN` 确保只返回有版本记录的游戏：

```sql
INNER JOIN cc_logic_game_version v ON w.version = v.version
```

| 字段 | 说明 |
|------|------|
| `version` | 版本号（与 `cc_logic_game_wide.version` 关联） |

---

## 关联关系图

```
cc_logic_game (g)
├── cc_logic_game_wide (w)  ON w.logic_game_id = g.id
│   ├── cc_logic_game_version (v)  ON w.version = v.version
│   └── cc_promotion_channel (pc)  ON pc.game_tag = w.channel_game_tag
├── cc_game (ga)  ON ga.logic_game_id = g.id
├── cc_logic_game_exclude (e)  ON e.logic_game_id = g.id (排除)
└── cc_logic_game_blacklist (bl)  ON bl.logic_game_id = g.id (黑名单)
```
