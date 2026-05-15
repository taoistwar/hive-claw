# GPUI 集成状态说明

## 背景

GPUI 是 Zed 编辑器的内部 GUI 框架，具有高性能和原生体验的优势。但在本项目集成过程中遇到以下挑战：

## 技术挑战

### 1. API 复杂性

GPUI 的 `Application` 初始化需要多个依赖：

```rust
// GPUI 需要复杂的初始化流程
let app = Application::with_platform(platform)
    .with_assets(asset_source)
    .with_http_client(http_client);
```

需要实现或注入：
- `Platform` trait 实现（Linux: Wayland/X11, macOS: Cocoa, Windows: DirectComposition）
- `AssetSource` trait 实现（资源加载）
- `HttpClient` trait 实现（网络请求）

### 2. Render Trait 签名

GPUI 的 `Render` trait 需要 3 个参数：

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // GPUI 使用独特的布局系统（Flexbox 风格）
        div()
            .flex()
            .child(...)
    }
}
```

### 3. 文档稀缺

GPUI 是 Zed 内部框架，主要服务于 Zed 编辑器本身：
- 没有独立的官方文档
- API 随时可能变化
- 示例代码主要在 Zed 源码中

## 当前状态

### TUI 版本（推荐）

✅ **已实现并可用**

```bash
cargo build --release
./target/release/offline-analysis-agent --gui tui
```

**优势**：
- 跨平台（Linux/macOS/Windows）
- 无需图形环境（SSH 可用）
- 编译快速（~30 秒）
- 二进制小（~2MB）
- 依赖稳定（ratatui 0.24）

### GPUI 版本（实验性）

🚧 **依赖已配置，代码框架已编写**

**启用方法**（需要图形环境）：

```toml
# Cargo.toml
gpui = { git = "https://github.com/zed-industries/zed.git", branch = "main" }
# ratatui = "0.24"  # 注释掉
# crossterm = "0.27" # 注释掉
```

**系统依赖**：
```bash
# Ubuntu/Debian
apt-get install libwayland-dev libxkbcommon-dev libgtk-3-dev libclang-dev

# macOS
xcode-select --install
```

## 建议

### 短期（MVP 阶段）

使用 TUI 版本交付，专注于核心功能：
- Azkaban 任务管理
- Hive 元数据同步
- AI 辅助生成
- Git 自动化
- 数据质量检查

### 中期（优化阶段）

如确有桌面 GUI 需求，考虑以下方案：

1. **继续探索 GPUI**
   - 投入时间深入研究 Zed 源码
   - 实现正确的 Platform 和 AssetSource
   - 预计需要 3-5 天

2. **替代方案：egui**
   - 更简单的即时模式 GUI
   - 文档丰富，社区活跃
   - 跨平台支持良好

3. **替代方案：Tauri**
   - Web 技术栈（React/Vue + Rust）
   - 原生性能和外观
   - 开发体验优秀

### 长期（产品化阶段）

根据用户反馈决定：
- 如果用户主要在服务器环境使用 → 保持 TUI
- 如果用户需要桌面体验 → 投入 GPUI 或迁移到 egui/Tauri

## 文件位置

- `src/gui/app.rs` - TUI 实现（当前使用）
- `src/gui/app.rs.gpui.bak` - GPUI 实现草稿（备份）
- `Cargo.toml` - GPUI 依赖已配置但注释

## 编译对比

| 项目 | TUI | GPUI |
|------|-----|------|
| 编译时间 | ~30 秒 | ~10 分钟 |
| 二进制大小 | ~2MB | ~50MB+ |
| 依赖数量 | ~150 | ~500+ |
| 运行环境 | 任意终端 | X11/Wayland |
| 开发难度 | 低 | 高 |

## 结论

**当前决策**：TUI 作为默认和主要 GUI 方案，GPUI 保留为可选实验性功能。

这一决策基于：
1. MVP 时间约束（5 天）
2. 目标用户环境（主要是服务器）
3. 开发资源有限
4. GPUI 学习曲线陡峭

如后续有明确需求，可重新评估 GPUI 或其他 GUI 方案。
