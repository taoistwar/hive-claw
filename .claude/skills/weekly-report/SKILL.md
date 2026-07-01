---
name: weekly-report
description: Generate a structured weekly report from git commit history, grouping commits by functional area.
argument-hint: "[week-range|YYYY-MM-DD..YYYY-MM-DD|since=YYYY-MM-DD]"
user-invocable: true
disable-model-invocation: false
---

# 周报生成

读取 git log，按工作内容分组，生成周报表写入 `WEEK.md`。

## 参数

`$ARGUMENTS` 可选：不传默认最近 7 天，支持 `YYYY-MM-DD..YYYY-MM-DD` 或 `since=YYYY-MM-DD`。

## 流程

1. `git log --since="<start>" --until="<end>" --format="%ad %h %s" --date=short | sort` 获取提交
2. `git config user.name` 获取作者
3. 读提交消息，按**语义**分组——同一个功能域的放一组，不要机械按 scope 标签切分
4. 每组一行写入表格，每行格式：

```markdown
| 项目 | JIRA | 需求描述 | 负责人 | 时间计划 | 本周进展 / Block问题 |
|------|------|---------|--------|---------|---------------------|
| HiveClaw | - | <功能域：一句话> | <作者> | - | <用中文概括这组提交做了什么，关键提交带 hash>。**无 Block。** |
```

5. 表格底部加一行汇总：总提交数、日期范围、覆盖的功能域。

## 要点

- **分组看内容不看标签**：提交的 scope 前缀只是参考，真正决定分组的是它实际改了什么东西
- **描述给人看**：不是罗列 commit message 翻译，而是写清楚这件事情做完后的结果
- **技术名词保留英文**：AgentContext、WASM、Redis、musl 等不用硬翻
- **有 Block 才写 Block**：没遇到阻塞就写"无 Block"
- **提交太多时归纳**：超过 50 个提交，每组只提单最关键的
