# Facet — Specification

**Facet** (working name) is a production-grade native Git GUI for Windows 11, written entirely in Rust.

The owner is a professional game developer working with Unreal/Unity-scale repositories: 100k+ files, huge
binaries, Git LFS. **SourceTree is the baseline — beat it decisively on speed, correctness, and visual clarity.**
Goal: looks like Apple designed it, runs like Sublime Merge.

This document is the complete, authoritative requirements list. `PLAN.md` holds status, architecture decisions,
the module map, gotchas, and next steps. **A new session needs nothing beyond SPEC.md and PLAN.md.**

---

## 1. Session protocol (strict)

1. The project is built **one milestone per session**. Never begin the next milestone in the same session, even
   if context or budget remains.
2. At the start of every session:
   - Read `SPEC.md` and `PLAN.md` first.
   - Verify `cargo build` and `cargo test` pass **before changing anything**. If the baseline is broken, fix
     that first.
   - Implement **only** the next milestone (the one PLAN.md names as next).
   - Honor every gotcha recorded in PLAN.md.
3. Keep both docs current while working:
   - `SPEC.md` — requirements (this file). Change it only when the owner changes requirements.
   - `PLAN.md` — milestone status, architecture decisions with rationale, module map, known issues/gotchas,
     next steps.
4. A milestone is **done** only when it compiles, runs, passes tests, and `cargo clippy` is clean
   (`cargo clippy --workspace --all-targets -- -D warnings`).
5. When the milestone is done: update PLAN.md, then end the session with **exactly three outputs**:
   1. A short **manual test checklist** — what the owner should click/verify in the running app.
   2. A suggested **git commit message**.
   3. A **next-session kickoff prompt** in a fenced code block, ready to paste into a fresh session. Keep it
      under ~150 words; the docs carry the detail. It must instruct the next session to:
      - read SPEC.md and PLAN.md first;
      - verify `cargo build` + `cargo test` pass before changing anything (fix baseline breakage first if not);
      - implement only the next milestone, **named explicitly**;
      - honor any gotchas recorded in PLAN.md;
      - follow this same session protocol — **including generating its own handoff prompt at the end**.
6. The protocol must **self-propagate**. The owner will never re-paste the original master prompt; every
   session's kickoff prompt must carry the rules above forward.
7. Profile before claiming any performance target is met. Report measured numbers, and say plainly when a
   budget has not been measured yet.

---

## 2. Non-negotiables

### 2.1 Stack
- **100% Rust**, GPU-rendered native UI.
- Stack: **eframe/egui (wgpu backend)**, **tokio**, **notify**, **syntect**, **image**, **arboard**, the
  **windows** crate.
- **No webview, no Electron, no libgit2** (and no gitoxide-as-a-backend: git behavior comes from the git CLI).

### 2.2 Git access
- All git operations go through async **git CLI plumbing** (`status --porcelain=v2 -z`,
  `log -z --format=…`, `for-each-ref`, etc.).
- **Never parse human-readable output.** Use `-z` / NUL-delimited and explicit `--format` strings.
- This guarantees parity with the owner's hooks, LFS, credential manager, GPG/SSH signing, autocrlf, and long
  paths.
- **Never bypass hooks** (no `--no-verify`, no hook-skipping plumbing shortcuts for user-visible operations).
- Offer (per-repo, opt-in) to enable `core.fsmonitor` and `core.untrackedCache`.
- **Never modify the global git config.** Per-repo config only, and only with consent.

### 2.3 Responsiveness
- **The UI thread never blocks.** Git work runs on tokio, is cancellable, and streams results in.
- A debounced (~150 ms) filesystem watcher triggers **targeted refreshes, never full reloads**.

### 2.4 Performance budgets
Verified against a **generated benchmark repo (100k files, 100k commits)**:

| Budget | Target |
| --- | --- |
| Cold start | < 1 s |
| Warm status refresh | < 150 ms |
| Commit log scrolling | 120 fps via virtualization + pagination |
| 10 MB diffs | render lazily |
| Idle RAM | < 300 MB |

### 2.5 Safety
- Confirm destructive operations, and make them **undoable**: auto-stash before discard/reset/rebase;
  reflog-backed undo.
- Never modify the global git config.

### 2.6 Quality bar
- **No stubs or placebo buttons:** every visible control works or doesn't ship.
- **Integration tests for every git operation** against temp repos.
- **Golden tests for all output parsers.**

---

## 3. Design language (Apple-clean, Windows-native)

- **Typography:** Segoe UI Variable.
- **Layout rhythm:** 8pt spacing grid, generous whitespace, hairline dividers, **one accent color**, zero
  nested-box clutter.
- **Window:** custom title bar with **native caption buttons**, **Mica backdrop** via
  `DwmSetWindowAttribute`, **per-monitor DPI v2**, dark/light **follows system**.
- **Layout:** slim collapsible **sidebar** (branches, stashes, tags, remotes, LFS locks, submodules, worktrees)
  · **file/commit list** · **diff pane**; resizable, **layout remembered**.
- **Motion:** subtle 120–150 ms ease-out. **Skeleton loading instead of spinners.** Inline non-modal errors.
  Undo toasts.
- **Keyboard:** command palette (**Ctrl+P**) reaches **every action**; full keyboard navigation; shortcuts
  shown in tooltips.
- **Transparency of operation:** a **"command log" panel** showing the exact git commands executed.

---

## 4. Features (all required, in milestone order)

### 4.1 Daily
- Status.
- Stage/unstage by **file, hunk, and line**.
- Discard **with undo**.
- Commit / amend.
- Syntax-highlighted diffs with **word-level intra-line highlighting**.
- **Image diff** (side-by-side / swipe / onion-skin) for png/jpg/tga/bmp/webp.
- **Binary-aware handling** (.uasset, .umap, .fbx, .psd, .blend → metadata/thumbnail, **never text diff**).

### 4.2 History & remotes
- Virtualized commit graph.
- Search (message / author / file / pickaxe).
- Blame.
- File history with rename following.
- Branch / checkout / merge.
- Tags.
- Stash, including partial.
- Fetch / pull (rebase or ff-only) / push (force-with-lease).
- Multiple remotes.

### 4.3 Power
- Interactive rebase (drag to reorder, squash/fixup/edit/drop).
- Cherry-pick, revert, reset.
- Reflog browser + undo.
- 3-way conflict editor, plus configurable **per-extension external diff/merge tools** (UnityYAMLMerge,
  P4Merge, Beyond Compare).
- Worktrees, submodules.
- Must not break **sparse-checkout** or **partial clones**.

### 4.4 Game dev
- Full **LFS** support (transfer progress, pointer awareness).
- **LFS file locking** (lock/unlock/list, locked-by-whom indicators).
- Unity/Unreal `.gitignore` templates.
- Huge-repo performance as a first-class requirement.

### 4.5 Integration
- Multi-repo tabs.
- Open in Windows Terminal / Explorer / VS Code / Rider.
- GitHub / GitLab / Azure DevOps link-outs for commits and PRs.
- Windows toast notifications for long operations.
- Settings for theme, fonts, external tools, git path.

---

## 5. Milestones

| # | Scope |
| --- | --- |
| **M0** | Scaffold, SPEC.md + PLAN.md, theme system, Mica window, repo open dialog, command palette shell |
| **M1** | Status, staging (file/hunk/line), text + image diffs, commit/amend, discard + undo |
| **M2** | Commit graph, branches, remotes, stash |
| **M3** | Interactive rebase, conflicts, blame, search, reflog/undo |
| **M4** | LFS + locking, submodules, worktrees, settings, perf pass, MSIX installer + single-exe release build |

Features from §4 that the milestone table does not name explicitly (search subsets, tags, cherry-pick/revert/
reset, external tools, gitignore templates, link-outs, toast notifications, open-in integrations) are assigned
to milestones in PLAN.md; every one of them must be shipped by the end of M4.
