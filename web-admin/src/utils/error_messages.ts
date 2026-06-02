// Centralized error code → user-facing message mapping.
// Codes from specs/003-admin-center / 004-agent-runtime / 006-game-alias / 008-agent-hook

const ERROR_MESSAGES: Record<number, string> = {
  // Auth (1xxx)
  1001: '密码错误',
  1002: '账号已禁用',
  1003: '账号已锁定',
  1004: 'Token 无效或已过期',
  1005: '非管理员账号',

  // Permission (2xxx)
  2001: '权限不足',

  // Admin (3xxx)
  3001: '管理员不存在',
  3002: '手机号已存在',
  3003: '不可删除超级管理员',
  3004: '不可禁用最后一个超级管理员',
  3008: '新密码与旧密码相同',

  // Generic (4xxx / 5xxx)
  4000: '请求参数错误',
  4040: '资源不存在',
  4090: '资源冲突',
  5000: '服务器内部错误',

  // 004 Agent Runtime
  4030: '能力调用被拒绝',
  4045: '未知能力',
  4091: '标签正在使用中',
  4092: 'DAG 存在环路',
  4093: '资源被引用，无法删除',
  4094: '资源已被他人修改，请刷新后重试',
  4291: '聊天并发超出上限',
  5001: '入口 Agent 不可删除',
  5002: 'Schema 不匹配',
  5003: '能力调用被拒绝',
  5004: 'Plugin 调用超时',
  5005: 'Workflow 映射无效',
  5006: 'Agent 嵌套深度超限',
  5007: '模型配置不存在',
  5008: '内置 Skill 不可修改',
  5009: 'Plugin 池繁忙',
  5010: '内置 Tool 不可修改',

  // 006 Game Alias
  4001: '游戏别名不存在',
  4002: '游戏名称已存在',
  4003: '游戏名称为空',
  4004: '游戏名称过长',
  4005: '别名为空',
  4006: '别名过长',
  4007: '别名数量过多',
  4008: '别名已被使用',

  // 008 Agent Hook
  6001: '该触发点最多配置 5 个 Hook',
  6002: 'Hook 引用的 Function/Workflow 不存在或已删除',
  6003: 'Webhook URL 不合法（仅支持 HTTPS 且不允许内网地址）',
  6004: 'Hook 执行超时',
  6005: 'Hook（阻塞模式）执行失败，Agent 流程已中止',
  6006: 'Hook 配置不存在',
};

export function getErrorMessage(code: number): string {
  return ERROR_MESSAGES[code] ?? `未知错误 (code=${code})`;
}
