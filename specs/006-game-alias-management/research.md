# Research: Game Alias Management

**Created**: 2026-06-01  
**Feature**: Game Alias Management (006-game-alias-management)

## Technical Decisions

### 1. One-to-Many Data Model for Game Aliases

**Decision**: Two-table design — `games` (parent) + `game_alias_entries` (child, one row per alias)

**Rationale**:
- `alias` column can have a UNIQUE index at DB level, enforcing global uniqueness across ALL games
- `WHERE alias = ?` is efficient (B-tree index), no JSON traversal needed
- Natural SQL for search: `games.name LIKE ? OR game_alias_entries.alias LIKE ?`
- Easy to manage individual aliases (add/remove without rewriting entire JSON array)
- Transaction-safe: insert game + all aliases in one transaction, rollback on any conflict

**Alternatives Considered**:
- JSON column: cannot enforce global uniqueness, requires application-level check of all rows, `JSON_SEARCH` is slower than B-tree index
- Three-table normalized (aliases + mapping): over-engineered for this use case; alias text is short (50 chars), duplication in `game_alias_entries` is acceptable

**Implementation**:
- `games`: id (BIGINT PK), name (VARCHAR(50) UNIQUE), created_at, updated_at
- `game_alias_entries`: id (BIGINT PK), game_id (BIGINT FK → games.id ON DELETE CASCADE), alias (VARCHAR(50) UNIQUE)
- Insert: transaction — INSERT game → INSERT N alias rows; if any UNIQUE conflict, rollback
- Update: DELETE existing alias rows → INSERT new alias rows within transaction
- Search: `SELECT g.*, JSON_ARRAYAGG(gae.alias) AS aliases FROM games g LEFT JOIN game_alias_entries gae ON gae.game_id = g.id WHERE g.name LIKE ? OR EXISTS (SELECT 1 FROM game_alias_entries WHERE game_id = g.id AND alias LIKE ?) GROUP BY g.id`

### 2. Reuse Existing Auth & RBAC Infrastructure

**Decision**: Reuse the existing admin center authentication middleware, role definitions, and permission checking patterns

**Rationale**:
- Feature is a sub-module of the admin center (spec assumption)
- Existing roles (Normal=1, System=2, Super=3) already define the needed permission model
- Avoids duplicating auth logic
- Consistent with Principle V (Simplicity & YAGNI)

**Implementation**:
- Same JWT middleware for authentication
- Same error code 2001 for insufficient permissions
- Normal role: only read access
- System/Super roles: full CRUD access

### 3. API Design Pattern

**Decision**: Follow existing admin center API conventions (`/api/game-aliases`)

**Rationale**:
- Consistent with established RESTful patterns in 003-admin-center
- Same response envelope `{ code, data, message }`
- Same error code ranges (4001-4999 for game alias management)

**Implementation**:
- Resource path: `/api/game-aliases`
- New error code range: 4001-4999
- Pagination: same `page` / `page_size` query parameters
- Search: `?q=keyword` parameter matching both name and alias

### 4. Search Implementation

**Decision**: `LIKE` on `games.name` + `LIKE` on `game_alias_entries.alias` with EXISTS subquery

**Rationale**:
- Data volume limited to ~1000 games × ~20 aliases = ~20k alias rows
- B-tree index on `name` and `alias` columns makes `LIKE '%keyword%'` efficient enough
- EXISTS subquery avoids full join for search count
- Can be combined with OR for unified search

**Implementation**:
- `WHERE g.name LIKE '%q%' OR EXISTS (SELECT 1 FROM game_alias_entries WHERE game_id = g.id AND alias LIKE '%q%')`

### 5. Alias Deduplication Strategy

**Decision**: Frontend deduplication on submit + Backend deduplication as authoritative check

**Rationale**:
- Frontend: good UX — user doesn't see errors for accidental duplicates they typed
- Backend: security — DB UNIQUE constraint is the final authority
- Cross-game uniqueness: enforced by `alias` UNIQUE index on `game_alias_entries`

**Implementation**:
- Frontend: `Set` to deduplicate before submit, trim whitespace
- Backend: deduplicate in service layer before SQL, UNIQUE index catches edge cases
- If cross-game alias conflict: return `ALIAS_ALREADY_IN_USE` error

### 6. Audit Logging

**Decision**: Reuse existing audit logging pattern from 003-admin-center

**Rationale**:
- spec FR-016 requires audit logging for create/update/delete
- Existing `audit_logs` table and `services/audit.rs` already handle this
- Just add new operation types for game aliases

**Implementation**:
- Operation types: `GAME_ALIAS_CREATE`, `GAME_ALIAS_UPDATE`, `GAME_ALIAS_DELETE`
- Target ID: game ID
- Reuse same `record()` function

## Security Considerations

1. **Input Validation**: Name and each alias validated for length (max 50 chars), non-empty
2. **Array Size Limit**: Max 20 aliases per game, enforced at API level
3. **Uniqueness**: Game name UNIQUE + alias UNIQUE at DB level (transactional)
4. **RBAC**: All write operations require System or Super role
5. **XSS Prevention**: Frontend renders aliases as text tags, not HTML
6. **SQL Injection**: All queries use SQLx parameterized bindings

## Performance Considerations

1. **Indexes**: UNIQUE index on `games.name`, UNIQUE index on `game_alias_entries.alias`
2. **Pagination**: Required for list endpoint (max 100 per page)
3. **No N+1**: Single query with `LEFT JOIN` + `JSON_ARRAYAGG` to fetch game + all aliases in one query
4. **Transaction scope**: Insert/delete alias rows within same transaction as game row operations
