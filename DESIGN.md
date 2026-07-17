# Design

## Source of truth
- Status: Active
- Last refreshed: 2026-07-16
- Primary product surfaces: macOS desktop shell, workspace sidebar and file tree, source editor chrome, preview chrome, modal dialogs, pane popouts.
- Evidence reviewed: `README.md`, `UI-skin.md`, `src/App.tsx`, shell and preview components, `src/lib/theme.ts`, `src/lib/themeRuntime.ts`, `src/styles/base.css`, `src/styles/app-shell.css`, `src/styles/responsive.css`, `src/styles/markdown-preview.css`, theme/shell tests, and Tauri native-menu commands.
- Rendering and skin boundary: root skin tokens may change application chrome, CodeMirror, Markdown preview, dialogs, Mermaid output, and Excalidraw chrome. Markdown parsing, preprocessing, rendered semantics, export behavior, media/document pixels, and editable Excalidraw scene data remain unchanged.

## Brand
- Personality: quiet, precise, local-first, and familiar to a macOS user.
- Trust signals: native system menus, restrained materials, predictable file operations, visible document state, strong keyboard and focus behavior, and explicit destructive confirmations.
- Avoid: marketing-style hero composition, oversized branding, decorative gradients, warm parchment-heavy chrome, floating card layouts, excessive pills, novelty animation, and controls that look web-only.

## Product goals
- Goals: make workspace management feel close to Finder and native macOS document apps; keep common file operations discoverable and fast; provide direct manipulation for rename and move; provide five coherent application skins without changing document semantics or state.
- Non-goals: changing Markdown parsing or output, replacing CodeMirror, redesigning PDF/DOCX/media renderers, adding cloud sync, adding multi-workspace tabs, or introducing a new dependency/design-system package.
- Success signals: users can create, rename, delete, and move entries without hunting; selected/drop/rename states are unambiguous; file operations remain keyboard accessible; skin changes are immediate and persistent without remounting the editor; Markdown structure and behavior remain unchanged while its visual tokens follow the active skin.

## Personas and jobs
- Primary personas: writers, developers, and technical note-takers working with local folders on macOS.
- User jobs: open a folder, scan its hierarchy, create notes and folders, rename entries in place, reorganize with drag and drop or a destination dialog, delete with confidence, edit source, and compare live output.
- Key contexts of use: repeated desktop writing sessions, keyboard-heavy editing, mixed Markdown/HTML/media workspaces, and external file changes from Git or other tools.

## Information architecture
- Primary navigation: native application menu plus the persistent workspace source list.
- Core routes/screens: one main editor window, optional editor/preview popouts, modal decision sheets.
- Content hierarchy: compact application toolbar; workspace source list; editor/preview pane headers; document content; modal decisions above all ordinary actions.

## Design principles
- Direct before modal: use selection, inline rename, disclosure controls, and drag/drop for reversible organization; reserve modal dialogs for creation details, destination choice, destructive confirmation, conflicts, and errors.
- Familiar macOS grammar: source-list rows, blue selection, system typography, compact icon controls, Return/Escape editing, Command-Delete deletion, contextual menus, and sheet-like dialogs.
- Document first: chrome stays quiet and compact so source and rendered content carry the visual weight.
- State is explicit: selected, open, edited, busy, invalid drop, and destructive states must be visually and semantically distinct.
- Tradeoffs: single selection is preferred over multi-select in this pass; permanent deletion remains explicit instead of being mislabeled as moving to Trash.

## Visual language
- Color: five token-complete Chinese-inspired palettes cover chrome, editor, preview, controls, and diagrams; semantic red/yellow/green and varied file-kind icon colors remain recognizable in every skin.
- Typography: `-apple-system`/BlinkMacSystemFont for chrome; established CodeMirror and Markdown font roles, metrics, and hierarchy remain unchanged across skins.
- Spacing/layout rhythm: 4px base rhythm; compact 28-32px controls and rows; 44-48px primary toolbar; 38-40px pane headers.
- Shape/radius/elevation: 5-8px radii for controls and rows, 10px maximum for dialogs, hairline borders, and subtle menu/dialog shadows only.
- Motion: 120-160ms color/opacity transitions for hover and drop feedback; no layout-shifting animation.
- Imagery/iconography: existing Lucide icons, used as familiar symbols with tooltips and accessible names.

## Components
- Existing components to reuse: `AppToolbar`, `FileSidebar`, `FileTreeRows`, `WorkspaceEntryDialog`, `FeedbackDialog`, `PaneHeader`, existing modal priority in `App.tsx`, and existing Tauri mutation receipts.
- New/changed components: compact toolbar state presentation; sidebar action menu; roving single-selection tree rows; inline rename editor; drag/drop targets; `WorkspaceMoveDialog`; reusable move-destination/path policy helpers.
- Variants and states: root/folder/file targets; selected/open/drop-target/dragging/renaming; clean/edited/busy; default/destructive dialog; valid/invalid destination.
- Token/component ownership: shell tokens live in `src/styles/base.css`; application chrome and component states live in `src/styles/app-shell.css`; narrow-window adaptations live in `src/styles/responsive.css`.

## Accessibility
- Target standard: WCAG 2.1 AA for the webview surface.
- Keyboard/focus behavior: one tabbable tree row at a time; arrows navigate disclosure state; Return/F2 starts rename where allowed; Escape cancels; Command-Delete requests deletion; context-menu key and Shift-F10 remain supported; move has a non-drag modal alternative.
- Contrast/readability: selected rows use high-contrast text; focus rings do not rely on color alone; destructive controls retain text labels.
- Screen-reader semantics: preserve `tree`, `treeitem`, `aria-expanded`, `aria-selected`, dialog naming, live status, and descriptive icon labels.
- Reduced motion and sensory considerations: transitions are nonessential and disabled under `prefers-reduced-motion`; no flashing or large parallax effects.

## Responsive behavior
- Supported breakpoints/devices: desktop-first Tauri windows, with existing 980px and 640px adaptations retained.
- Layout adaptations: sidebar narrows before panes stack; collapsed rail remains icon-led; action labels may hide but accessible names remain; document previews retain their established full-width behavior.
- Touch/hover differences: hover is supplemental; every command remains available by click/tap, keyboard, or menu.

## Interaction states
- Loading: global document state reads `Working`; file-operation controls disable while their mutation is active.
- Empty: sidebar provides a concise empty workspace message and keeps the open-directory path in the native File menu.
- Error: user-friendly modal feedback only; no inline technical dumps or toast banners.
- Success: ordinary file mutations stay silent and update the tree/selection immediately from the mutation receipt.
- Disabled: controls reduce contrast and expose native disabled semantics without disappearing.
- Offline/slow network: not applicable; all core operations are local, but slow filesystem operations use the busy state.

## Content voice
- Tone: concise, factual, desktop-native.
- Terminology: Workspace, New Markdown File, New Folder, Rename, Move, Delete, Saved, Edited, Working.
- Microcopy rules: use ellipses for commands that open a dialog; never call permanent deletion “Move to Trash”; avoid implementation terminology in user-facing errors.

## Implementation constraints
- Framework/styling system: React 19, strict TypeScript, plain CSS, Lucide React, Tauri 2 and Rust.
- Design-token constraints: extend the existing CSS files; do not add a design-system dependency or utility framework.
- Performance constraints: tree interactions must remain synchronous; drag feedback must not trigger filesystem work until drop; no document re-rendering solely for chrome changes.
- Compatibility constraints: preserve Tauri authorization boundaries, mutation receipts, popout replication, external-file monitoring, and native menu behavior.
- Rendering constraint: skin work may replace visual values in `markdown-preview.css` with semantic tokens, but must not change Markdown parser/preprocessor/render components, generated document structure, export themes, content filters, or user-authored media and Excalidraw scene colors.
- Test/screenshot expectations: TDD for new behavior; frontend unit tests for tree policies/interactions; Rust tests for move authorization and path rules; full frontend/Rust gates; Playwright or macOS screenshots at desktop and narrow sizes before completion.

## Open questions
- [ ] Future localization strategy for the currently mixed English/Chinese UI / product owner / affects copy consistency, not this interaction pass.
- [ ] Whether deletion should use the macOS Trash in a future release / product and security review / would change recovery semantics and backend permissions.
- [ ] Whether multi-selection and batch move/delete are needed / product owner / intentionally excluded from this pass.
- [ ] Whether exported HTML should optionally follow the application skin / product owner / export remains independent in this release.
