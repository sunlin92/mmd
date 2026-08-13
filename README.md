# MMD

MMD 是一个本地优先的桌面 Markdown 编辑器，基于 **Tauri 2 + React + TypeScript + Vite** 构建。它面向类似 Typora 的轻量写作体验：左侧管理本地 Markdown 工作区，中间编辑源码，右侧实时预览，并尽量使用原生桌面应用的交互方式。

## 已交付

- **本地文件读写**：通过 Tauri 原生文件/目录选择器打开、保存、另存为 Markdown、HTML 和 Excalidraw 文件。
- **原生系统菜单栏**：`File` 菜单提供 `New`、`Open File…`、`Open Directory…`、`Save`、`Save As…` 等操作，并支持常见快捷键。
- **工作区目录树与大纲**：打开本地目录后展示文件树，支持折叠/展开、右键菜单、新建文件/文件夹、重命名、删除、刷新，以及将图片或音频拖入 Markdown 编辑器插入相对引用；同级的大纲标签按 H1-H6 显示文档结构，并同步定位编辑器和预览。
- **工作区快速打开与全文搜索**：`Cmd/Ctrl + P` 按文件名快速定位工作区文件，`Cmd/Ctrl + Shift + F` 搜索 Markdown 内容；索引仅保存在内存中，可从搜索面板重建，也可在设置中丢弃或重建。
- **编辑器 + 实时预览**：源码编辑区与 Live Preview 可拖拽调整宽度，并支持单独弹出独立窗口；编辑器底部实时显示字数、字符数、行数和光标位置。
- **未保存保护**：关闭程序时如存在未保存内容，会弹出保存/取消/退出确认框；弹出窗口会随主程序一起关闭。
- **会话恢复**：重新打开应用时自动恢复上次的工作区目录，以及其中最后一个已提交保存的文件；未保存草稿不会写入会话记录。
- **Markdown 渲染增强**：支持 GFM、CJK 友好解析、硬换行、数学公式/KaTeX、Mermaid 图表、GitHub alerts、代码高亮/复制、标题标点处理和自适应本地图片。
- **五套本地主题**：锦绣朱砂、汝窑天青、青花霁蓝、松壑竹影和山水夜墨可从原生菜单切换，可跟随系统外观，并同步到主窗口、弹出窗口、Mermaid 与 Excalidraw。
- **版本化设置**：设置保存在应用数据目录，支持主题、语言、编辑区宽度等偏好，并能对损坏或未来版本数据执行重试/重置保护；自动保存、双向链接等开关不代表对应路线已全部交付。
- **Excalidraw 画布**：工作区可新建和编辑标准 `version: 2` `.excalidraw` 文件，保留元素、应用状态和嵌入文件；画布资源与字体随应用本地发布，默认透明背景和中文文本字体 `fontFamily: 5`。
- **富文本与资源管线**：Word/HTML/RTF/PDF 剪贴板内容会经过清理并转换为 Markdown；剪贴板图片按 MD5 去重，可写入工作区相对目录或用户显式授权的绝对目录。
- **关联 Excalidraw 资源**：可向 Markdown 插入与源场景关联的 SVG/PNG，源文件变化时校验并重新同步派生资源。
- **离线导出**：支持内联主题、字体和本地资源的单文件 HTML，1x/2x/3x 高清长图 PNG，以及包含源场景、SVG 和三种 PNG 倍率的 Excalidraw 资源包；导出前问题统一在模态框中反馈。
- **可配置快捷键**：保存、另存为、快速打开、全文搜索、导出和设置快捷键可修改，支持冲突检测、恢复默认，并同步到原生菜单。
- **应用更新**：启动时静默检查更新；发现新版本后可立即更新、稍后处理或跳过该版本，网络或运行时失败不会打断离线编辑。
- **统一弹框反馈**：错误、警告和需要用户选择的流程统一使用模态弹框；普通文件操作成功后保持静默。
- **系统打开入口**：打包配置为 `.md`、`.mdx`、`.markdown`、`.mdown` 和 `.mkd` 注册编辑器关联；支持 `mmd <file-or-directory>` 启动参数、系统“打开方式”/文件打开事件和单实例路由，后续请求会交给已运行的主窗口。
- **可信发布门禁**：GitHub Actions 对 macOS arm64/x64、Windows x64 和 Linux x64 执行原生构建、测试、打包与安装包冒烟；发布流程强制校验平台证书、macOS 公证凭据、更新签名密钥，并生成签名更新清单。
- **性能证据工具**：提供确定性 Markdown/Excalidraw 夹具、专业性能证据模板、体积采集器和失败关闭门禁；真实启动、内存与三平台产物数据仍需在目标机器和托管 CI 中采集。

## 实验性能力

- Mermaid 错误图表会安全回退为源码块，专用诊断占位仍在完善。
- PDF 和 DOCX 可在工作区预览；PDF 导出不属于当前承诺。
- 性能门禁与预算结构已经可复现，但尚不代表真实三平台产品性能已经达标。

## 计划路线

- 自动保存、崩溃草稿与统一的原子文档写入。
- 在托管发布中使用真实凭据验证签名、公证、更新安装与回滚。
- PDF 导出、Emoji 短代码、目录插入和 Excalidraw 字体缺失提示。
- 在目标机器上采集并审核启动、文档、画布、内存和三平台包体积证据。

后续功能、优先级与验收标准见 [ROADMAP.md](./ROADMAP.md)。

## 许可证

MMD 源代码以 [Apache License 2.0](./LICENSE) 发布。随应用分发的第三方组件继续适用各自许可证；精确文本位于 `public/vendor/notices/`，不会被项目根许可证覆盖或重新标注。

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

## CLI、单实例与文件关联

打包后的主程序接受一个文件或目录目标；以下示例假设 `mmd`（Windows 上为 `mmd.exe`）已位于 `PATH`：

```bash
mmd <file-or-directory>
mmd "./notes/draft.md"
mmd "./含 空格的项目"
mmd -- "./-draft.md"
```

- 相对路径按发起进程的当前工作目录解析。含空格或 Unicode 的路径应作为一个 shell 参数传入，最简单的方式是用引号包住完整路径；以 `-` 开头的目标应放在 `--` 之后。
- 文件目标进入独立文件打开流程；目录目标作为工作区打开。每次启动请求只接受一个目标，不支持其他命令行选项。
- 如果当前文档有未保存修改，MMD 会先显示保存、直接切换或取消的模态选择；干净文档会直接处理请求。
- 已运行 MMD 时，再次执行命令或通过系统打开文件不会创建第二套编辑会话，而是将请求依次交给现有主窗口并将其置于前台。重复的同一路径请求会合并。
- 如果目标在排队期间被移动、删除，或已经不再是常规文件/目录，MMD 不会提前授予访问权限，而会在主窗口中显示用户可读的模态错误。

文件关联来自打包清单，只覆盖 `.md`、`.mdx`、`.markdown`、`.mdown` 和 `.mkd`。关联需由安装包或桌面环境完成注册；`npm run tauri -- dev` 不会代表安装后的系统关联状态。不同平台及桌面环境可能保留已有默认应用或缓存关联，因此应通过已安装的对应平台包验证“打开方式”。打包程序不会保证把 `mmd` 自动加入 `PATH`；未配置 `PATH` 时请使用安装后的可执行文件完整路径。

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
npm run test:release-tools
npm run test:perf
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
3. 使用文件树顶部的新增菜单创建 Markdown 或 Excalidraw 文件；也可拖入工作区资源、粘贴富文本/图片，或插入与 Excalidraw 源场景关联的 SVG/PNG。
4. 使用 `Cmd/Ctrl + S` 保存，通过工具栏导出离线 HTML、高清 PNG 或 Excalidraw 资源包；快捷键可在设置中修改并恢复默认。
5. 点击 Editor 或 Live Preview 标题栏的弹出按钮可打开独立窗口；再次点击会聚焦已有窗口。

## 贡献约定

- 遵循 `AGENTS.md` 中的仓库约定。
- 使用 TSX 编写 React 组件，不使用 JSX 文件。
- 保持改动小而可验证，优先复用现有组件和工具函数。
- 不要降低 Tauri capability 或路径授权测试强度。
- UI 行为变更请更新对应单元测试，并在提交说明中列出验证命令。
