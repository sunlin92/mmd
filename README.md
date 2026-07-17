# MMD

MMD 是一个本地优先的桌面 Markdown 编辑器，基于 **Tauri 2 + React + TypeScript + Vite** 构建。它面向类似 Typora 的轻量写作体验：左侧管理本地 Markdown 工作区，中间编辑源码，右侧实时预览，并尽量使用原生桌面应用的交互方式。

## 功能特性

- **本地文件读写**：通过 Tauri 原生文件/目录选择器打开、保存、另存为 Markdown、HTML 和 Excalidraw 文件。
- **原生系统菜单栏**：`File` 菜单提供 `New`、`Open File…`、`Open Directory…`、`Save`、`Save As…` 等操作，并支持常见快捷键。
- **工作区目录树与大纲**：打开本地目录后展示文件树，支持折叠/展开、右键菜单、新建文件/文件夹、重命名、删除、刷新，以及将图片或音频拖入 Markdown 编辑器插入相对引用；同级的大纲标签按 H1-H6 显示文档结构，并同步定位编辑器和预览。
- **编辑器 + 实时预览**：源码编辑区与 Live Preview 可拖拽调整宽度，并支持单独弹出独立窗口；编辑器底部实时显示字数、字符数、行数和光标位置。
- **未保存保护**：关闭程序时如存在未保存内容，会弹出保存/取消/退出确认框；弹出窗口会随主程序一起关闭。
- **会话恢复**：重新打开应用时自动恢复上次的工作区目录，以及其中最后一个已提交保存的文件；未保存草稿不会写入会话记录。
- **Markdown 渲染增强**：移植并整理了父目录 `inkocean` 中的 Markdown 渲染逻辑，支持 GFM、CJK 友好解析、硬换行、数学公式/KaTeX、Mermaid 图表、GitHub alerts、代码高亮/复制、标题标点处理、自适应本地图片和 Typora Jinxiu 风格主题。
- **Excalidraw 画布**：工作区可新建和编辑标准 `version: 2` `.excalidraw` 文件，保留元素、应用状态和嵌入文件；画布资源与字体随应用本地发布，默认透明背景和中文文本字体 `fontFamily: 5`。
- **统一弹框反馈**：错误、警告和需要用户选择的流程统一使用模态弹框；普通文件操作成功后保持静默。

后续功能、优先级与验收标准见 [ROADMAP.md](./ROADMAP.md)。

## 项目结构

```text
src/                         React 前端
  App.tsx                    应用主编排
  components/                UI、编辑器、预览、目录树和弹框组件
  hooks/                     文档会话、窗体弹出、关闭保护、布局拖拽
  lib/                       Markdown 预处理、目录树、菜单事件、Tauri 命令封装
  styles/                    应用布局和 Markdown 预览样式
src-tauri/                   Tauri/Rust 后端
  src/commands.rs            文件/目录读写命令
  src/native_menu.rs         原生系统菜单栏
  src/path_auth.rs           本地路径授权与校验
  src/image_resolver.rs      本地 Markdown 图片解析
public/styles/typora-theme/  迁移的 Typora 主题资源
```

## 环境要求

- Node.js 与 npm
- Rust toolchain
- Tauri 2 所需的系统依赖

首次安装：

```bash
npm install
```

## 开发命令

```bash
npm run dev
```

启动 Vite 前端开发服务器。

```bash
npm run tauri -- dev
```

启动 Tauri 桌面开发模式。

```bash
npm run build
```

执行 TypeScript 编译并生成生产前端产物。

```bash
npm run tauri -- build
```

构建桌面应用安装包/可执行文件。

## 测试与质量门禁

本项目要求 TDD：新增功能或修复问题时，先写失败测试，再实现最小改动，最后重构并保持测试通过。

常用校验命令：

```bash
npm run lint
npm test
npm run typecheck
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
git diff --check
```

测试分布：

- 前端单元测试使用 Vitest，文件名为 `*.test.ts` 或 `*.test.tsx`。
- Rust 单元测试位于实现文件的 `#[cfg(test)]` 模块中。
- 文件系统授权、路径归一化、目录遍历、本地图片解析和关闭保护逻辑必须有测试覆盖。

## 安全模型

MMD 只访问用户主动选择过的文件或目录。Rust 后端维护会话级路径授权列表，所有读、写、目录刷新、图片解析和工作区变更都会进行路径归一化和授权检查。

安全限制包括：

- 拒绝未授权路径读写。
- 拒绝 `..` 父目录逃逸。
- 拒绝绝对路径、URL、`file:`、`data:` 等不受控图片引用。
- 拒绝符号链接/canonicalize 后逃出授权根目录。
- 不通过 shell 执行用户输入。

## 使用提示

1. 通过系统菜单栏 `File -> Open Directory…` 打开一个 Markdown 工作区。
2. 在左侧文件树中单击文件打开，使用同级 `Outline` 标签定位标题；右键文件或文件夹进行新建、重命名、删除等操作。
3. 使用文件树顶部的新增菜单创建 Markdown 或 Excalidraw 文件，也可将工作区图片或音频拖到编辑器中插入相对引用。
4. 使用 `Cmd/Ctrl + S` 保存，或 `File -> Save As…` 另存为新文件。
5. 点击 Editor 或 Live Preview 标题栏的弹出按钮可打开独立窗口；再次点击会聚焦已有窗口。

## 贡献约定

- 遵循 `AGENTS.md` 中的仓库约定。
- 使用 TSX 编写 React 组件，不使用 JSX 文件。
- 保持改动小而可验证，优先复用现有组件和工具函数。
- 不要降低 Tauri capability 或路径授权测试强度。
- UI 行为变更请更新对应单元测试，并在提交说明中列出验证命令。
