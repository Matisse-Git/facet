# Facet — Plan

Companion to `SPEC.md` (requirements + session protocol). This file is the working memory between
sessions: status, decisions, module map, gotchas, next steps. Keep it current.

## Milestone status

| # | Scope | Status |
| --- | --- | --- |
| M0 | Scaffold, docs, theme system, Mica window, repo open dialog, command palette shell | **done** (2026-11-17) |
| M1 | Status, staging (file/hunk/line), text + image diffs, commit/amend, discard + undo | **next** |
| M2 | Commit graph, branches, remotes, stash | not started |
| M3 | Interactive rebase, conflicts, blame, search, reflog/undo | not started |
| M4 | LFS + locking, submodules, worktrees, settings, perf pass, MSIX + single-exe release | not started |

The project folder is **not yet a git repository** — no `git init` / commit has been made by a
session (commits are the user's call).

## Build, run, test

```
cargo build --workspace                                   # debug (deps optimized)
cargo run -p facet [-- <repo path>]                       # debug run, console attached
cargo test --workspace                                    # unit + integration + golden
cargo clippy --workspace --all-targets -- -D warnings     # must be clean
cargo fmt --all
cargo build --release -p facet                            # thin LTO; use this for all profiling
cargo build --profile dist -p facet                       # fat LTO, ~10.5 min; shipping only (M4)
```

Requirements: Rust stable (built with 1.97.1), Git for Windows on `PATH` (tested with 2.45.1),
Windows 11. Build times on the dev machine: debug incremental ~25 s, `release` ~4 min, `dist` ~10.5 min.
Tests spawn the real `git` in temp directories with `GIT_CONFIG_GLOBAL` pointed at an empty file and
`GIT_CONFIG_NOSYSTEM=1`, so they never read or write the user's config.

Diagnostic environment variables:

| Variable | Effect |
| --- | --- |
| `FACET_CONFIG_DIR` | Use this folder instead of `%APPDATA%\Facet` for `settings.json` (isolates test runs). |
| `FACET_STARTUP_LOG` | File to which `first frame presented after N ms on <GPU adapter>` is written. |
| `WGPU_POWER_PREF` | `low` / `high` — override GPU adapter choice (see gotcha G4). |
| `WGPU_BACKEND` | `dx12` (default) / `vulkan`. Vulkan works but loses Mica (no alpha swapchain). |

## What M0 delivered

- Cargo workspace: `facet-git` (git plumbing library) + `facet` (the app).
- Window: custom 40 px title bar with **real DWM caption buttons** (snap layouts, hover states),
  Mica backdrop, per-monitor DPI v2 manifest, dark/light following the system (or forced), window
  placement + maximized state remembered across launches.
- Theme system: design tokens (`Palette` dark/light, spacing on a 4/8 grid, radii, motion), Segoe UI
  Variable (text/display optical sizes), Segoe Fluent Icons, Cascadia Mono.
- Repositories: native folder picker, drag-and-drop a folder, `facet <path>`, recent list, multi-repo
  tabs, session restore (open tabs + active tab), per-tab skeleton loading, overview of branch/HEAD
  commit, refresh on F5 and on window focus.
- Command palette (Ctrl+P): fuzzy search with highlighted matches over **every** action, including
  recent repositories, open tabs and theme switching.
- Command log panel (Ctrl+`): every git process with exact command line, duration, exit status,
  stderr on hover, click to copy.
- Inline non-modal error toasts; settings persisted atomically as JSON.
- 69 tests: 33 unit in `facet`; in `facet-git` 13 unit (parsers, command log), 22 integration
  against temp repos (13 repo, 9 runner) and 1 golden fixture.

## Architecture decisions (with rationale)

1. **Two crates.** `facet-git` has no UI dependency, so every git operation and parser is testable
   headlessly against temp repos. `facet` owns the window. Git logic never goes in `facet`.
2. **Git CLI only, via `Git`/`GitCommand` (runner.rs).** Fixed prefix `--no-pager -c color.ui=never`,
   `GIT_TERMINAL_PROMPT=0`, `CREATE_NO_WINDOW`, `kill_on_drop`. `.background()` adds
   `GIT_OPTIONAL_LOCKS=0` for refreshes so the app never fights the user's own git for `index.lock`.
   Non-zero exit is data in `output()`, an error in `run()`; `stream()` delivers stdout in chunks for
   large outputs. Cancellation through `CancellationToken`. Per-call config goes through `-c`, never
   `git config --global`.
3. **Parsers are pure functions over bytes** (`facet_git::parse::*`), each with a golden fixture in
   `tests/golden/*.bin` (marked `binary` in `.gitattributes` so NULs/line endings survive). Only `-z` /
   explicit `--format` output is parsed. The single sentence parsed is `git version`.
4. **`CommandLog` lives in the runner**, so nothing can run git without being logged. It is a bounded
   ring with a revision counter and a listener that requests a repaint.
5. **One `Action` enum (actions.rs).** Buttons, shortcuts and the palette all dispatch `Action`s into
   `FacetApp::run`. `actions::commands()` builds the palette list from current state, which is how the
   "palette reaches every action" rule is enforced; a unit test checks that every global shortcut has
   a palette entry. New features must add an `Action` + palette entry, not ad-hoc click handlers.
6. **Background work → `Event` over `std::sync::mpsc` + `ctx.request_repaint()`.** The UI thread only
   drains events; it never awaits, never touches disk except through `spawn_blocking`. Each tab holds
   a cancellation token for its in-flight load; a newer load cancels the older one. Blocking Win32
   dialogs (folder picker) run on their own thread.
7. **Reactive repaint.** No continuous animation loop: egui repaints on input, on events, and while an
   animation is in flight. Idle = zero frames. Keep it that way (it is what keeps idle CPU at 0).
8. **Hand-computed layout rects** (`ui/mod.rs`) instead of egui panels: title bar, sidebar, content,
   command log and splitters are rects derived from settings. It makes the title-bar hit-testing
   exact and keeps layout persistence trivial.
9. **Native chrome by subclassing the winit window** (`platform/windows.rs`): the client area is
   extended over the title bar (`WM_NCCALCSIZE`), `DwmDefWindowProc` gets first refusal on every
   message so the caption buttons stay real, and `WM_NCHITTEST` returns `HTCAPTION` except over
   rects the UI registers every frame (`Chrome::set_title_bar`). No fake min/max/close buttons.
10. **Mica needs a transparent swapchain**: DX12 + `Dx12SwapchainKind::DxgiFromVisual`
    (DirectComposition), transparent viewport, clear colour `[0,0,0,0]`,
    `DwmExtendFrameIntoClientArea(-1)`, `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW`. Panels that
    should be opaque paint their own background; the title bar and sidebar let Mica through.
11. **Window placement is persisted by Facet** (`Get/SetWindowPlacement`), not eframe
    (`persist_window: false`): winit's size math does not know about the reclaimed title bar and the
    window would creep by the caption height every launch.
12. **Settings**: one JSON file, tolerant load (unknown fields ignored, corrupt file renamed to
    `.corrupt` and defaults used), atomic save (temp file + rename), debounced 800 ms.
13. **Release profiles**: `release` = thin LTO (fast enough to iterate and profile on), `dist` = fat
    LTO + 1 codegen unit for shipping.
14. **wgpu device uses `MemoryHints::MemoryUsage`** (`main.rs`). The default (`Performance`)
    pre-sizes large allocator blocks and cost ~300 MB of private memory for nothing; Facet's draw
    load is tiny. Adapter selection is left at eframe's default (see G4).

## Module map

```
Cargo.toml                    workspace, shared deps, lints (clippy::all = deny), profiles
crates/facet-git/
  src/lib.rs                  public API re-exports
  src/runner.rs               Git, GitCommand (builder), GitOutput; spawn/stream/cancel
  src/cmdlog.rs               CommandLog, CommandRecord, CommandOutcome
  src/error.rs                GitError
  src/repo.rs                 RepoInfo, Head, version(), discover(), head()
  src/parse/mod.rs            shared helpers (utf8)
  src/parse/commit.rs         COMMIT_SUMMARY_FORMAT, CommitSummary, parse_commit_summary
  src/parse/version.rs        GitVersion, parse_version
  tests/common/mod.rs         TestRepo: isolated temp repo/bare repo, fixed identity + dates
  tests/repo.rs               discovery + HEAD integration tests
  tests/runner.rs             runner: streaming, stdin, cancel, logging, env
  tests/golden.rs + golden/   parser golden fixtures
crates/facet/
  build.rs                    embeds the app manifest (PerMonitorV2, long paths, UTF-8)
  src/main.rs                 runtime, wgpu/DX12 setup, viewport, eframe launch
  src/app.rs                  FacetApp state, Event, RepoTab, action dispatch, persistence
  src/actions.rs              Action enum, shortcuts, palette command list
  src/settings.rs             Settings (JSON), ThemeMode, recents
  src/theme.rs                tokens, Palette DARK/LIGHT, fonts, egui style install
  src/fuzzy.rs                fuzzy matcher for the palette
  src/format.rs               relative time, clock time, durations
  src/icon.rs                 procedural app icon (window/taskbar)
  src/platform/windows.rs     Chrome (subclass, Mica, caption, placement), folder picker, Explorer
  src/ui/mod.rs               frame layout, splitters, drop overlay
  src/ui/widgets.rs           buttons, icon buttons, elided text, skeleton, hairlines, tooltips
  src/ui/titlebar.rs          sidebar toggle, repo tabs, + button, drag region registration
  src/ui/sidebar.rs           recent repositories, git version footer
  src/ui/content.rs           welcome screen, repository overview, loading skeleton
  src/ui/command_log.rs       command log panel
  src/ui/palette.rs           command palette
  src/ui/toast.rs             toasts
```

## Where the remaining SPEC features land

Features in SPEC §2–§4 that the milestone one-liners do not name explicitly:

| Milestone | Also includes |
| --- | --- |
| M1 | FS watcher (`notify`, ~150 ms debounce → targeted refresh); syntax highlighting (`syntect`) + word-level intra-line diff; binary-aware handling (.uasset/.umap/.fbx/.psd/.blend → metadata, never text diff); lazy 10 MB diffs; clipboard via `arboard`; per-repo opt-in offer for `core.fsmonitor` / `core.untrackedCache`; **benchmark repo generator** (100k files / 100k commits) and the first real perf measurements (status refresh). |
| M2 | Tags; multiple remotes; fetch / pull (rebase or ff-only) / push (force-with-lease); credentials (askpass / GCM — see G7); process-tree kill via Job Objects (G6); partial stash; virtualized graph at 120 fps on the benchmark repo; Windows toast notifications for long operations; GitHub/GitLab/Azure DevOps link-outs. |
| M3 | Cherry-pick, revert, reset; file history with rename following; search (message/author/file/pickaxe); per-extension external diff/merge tools (UnityYAMLMerge, P4Merge, Beyond Compare); auto-stash + reflog-based undo for destructive operations. |
| M4 | Unity/Unreal `.gitignore` templates; open in Windows Terminal / VS Code / Rider; settings UI (theme, fonts, external tools, git path); sparse-checkout and partial-clone verification pass; embedded `.ico` + version resource; single-instance handling. |

## Gotchas (read before changing related code)

- **G1 — eframe/egui 0.37 API.** `App` has `logic(ctx, frame)` + `ui(ui, frame)` (no `update`).
  `Id::new` is deprecated → `Id::unique`; `ui.id()` → `ui.scope_id()`; `DroppedFile::path()` is a
  method; `TextEdit::frame` takes a `Frame`; variable-font axes go through `FontTweak::coords`.
  Do not blanket-replace `::new(` — `FontId::new` and `LayerId::new` are still correct.
- **G2 — `WM_ERASEBKGND` must paint black.** Without the black GDI fill (plus the
  `RedrawWindow(RDW_ERASE)` after install) the window is white instead of Mica and the caption
  buttons do not appear.
- **G3 — Mica only works on DX12 with `DxgiFromVisual`.** The HWND swapchain and Vulkan are opaque.
- **G4 — Do not force the low-power GPU.** On the dev machine (AMD integrated + GT 1030, monitor on
  the discrete card) `PowerPreference::LowPower` selects the integrated adapter and a driver thread
  spins at 100 % of a core while the app is idle (cross-adapter present). eframe's default
  (high performance) is idle at ~0 %. `main.rs` therefore leaves the preference alone. Any future
  "prefer integrated GPU" setting must be opt-in and measured.
- **G5 — every git process costs 50–90 ms here.** Process spawn dominates on Windows. Budget
  accordingly: run independent queries concurrently (`head()` does three at once), prefer one
  command with a richer format over several small ones, and never spawn per file/row.
- **G6 — cancellation kills `git.exe` only**, not its children (hooks, `git-lfs`, `ssh`). Before any
  long-running network or hook-running command ships (M2), spawn inside a Job Object with
  `KILL_ON_JOB_CLOSE`.
- **G7 — `GIT_TERMINAL_PROMPT=0` is set globally in the runner.** Good for M0/M1 (nothing may hang
  waiting for a console). Network operations in M2 need a credential path: let Git Credential
  Manager show its own UI and provide `GIT_ASKPASS`/`SSH_ASKPASS` for the rest.
- **G8 — hooks are never bypassed**: no `--no-verify`, no `-c core.hooksPath=`. Commit in M1 must
  surface hook output (stdout+stderr) in the UI and the command log.
- **G9 — never write global config.** Only `git -c k=v …` per call or `git config --local` after an
  explicit per-repo opt-in.
- **G10 — title-bar controls must register their rects** with `Chrome::set_title_bar` (physical
  pixels) or clicks on them become window drags. Leave `Chrome::caption_buttons()` width free on
  the right.
- **G11 — UI automation.** Synthetic key events need hardware scan codes (`MapVirtualKey`) or
  egui-winit ignores them; `SendKeys` alone is not enough for chords like Ctrl+\`. A force-killed
  instance keeps the GPU busy for a moment — measure startup with one clean launch at a time.
- **G12 — tooling.** Write source files with the editor tools, not shell heredocs (apostrophes in
  Rust break them silently). Fat LTO takes ~10.5 min — never use `--profile dist` in the edit loop.
- **G13 — `git log` on an unborn branch fails by design**; `head()` ignores that failure. Any new
  caller must treat "no commits yet" as a state, not an error.
- **G14 — session-restored tabs must not reorder the recent list** (`RepoTab::restored`); only
  user-initiated opens call `touch_recent`.

## Known issues / deliberate limits after M0

- True cold start (first launch after a reboot) has not been measured; only warm-cache launches.
- No file-system watcher yet: the overview refreshes on window focus and F5 only (watcher is M1).
- No single-instance guard: two Facet windows share one `settings.json`; last writer wins.
- An unborn branch shows one red (failed) `git log` row in the command log on each refresh. Honest,
  but noisy; M1's status-based refresh should avoid running it when there is no HEAD commit.
- Maximized window + auto-hide taskbar: the taskbar may not slide in (the classic custom-frame
  issue); needs the `ABM_GETAUTOHIDEBAR` one-pixel inset in `WM_NCCALCSIZE`.
- Many open tabs are squeezed, not scrolled; there is no overflow menu yet (the palette lists all
  tabs, so every tab stays reachable).
- The exe has no embedded `.ico`/version resource (the window/taskbar icon is set at runtime), so
  Explorer shows the generic icon for `facet.exe`.
- Bare repositories open and show the overview only.
- The folder picker dialog and drag-and-drop compile and are wired to the same `open_path` code
  path as the tested routes (`facet <path>`, recents, session restore), but were **not** exercised
  end-to-end in the M0 session (automation could not hold keyboard focus while the machine was in
  use). They are first on the manual checklist; fix before M1 work if either fails.
- The benchmark repository generator does not exist yet, so no budget that depends on it (status
  refresh, log scroll, large diffs) has been measured.

## Measurements

Dev machine: Windows 11 Home 26200, AMD integrated GPU + NVIDIA GT 1030, Git for Windows.
Method: `FACET_STARTUP_LOG` for time to first presented frame; `Get-Process` CPU time / working
set / private bytes sampled over a fixed idle window; one clean instance at a time.

Final M0 `release` build (thin LTO, 11.5 MB exe), one repository tab restored, 1296×839 window,
NVIDIA adapter on DX12:

| Budget (SPEC §2.4) | Measured | Verdict |
| --- | --- | --- |
| Cold start < 1 s | 842 / 887 ms to first presented frame (two launches, **warm disk cache**, while Unity shader compilers + Rider held the machine at 70–100 % CPU). Earlier on a quiet machine: 667 ms (fat-LTO build), 634 ms (debug). | Within budget warm. **True cold start (after reboot) not measured.** Startup is dominated by DX12 device + swapchain creation, not by Facet code. |
| Idle RAM < 300 MB | 163 MB working set, 224 MB private bytes | Met for the M0 shell. Re-measure every milestone; diff/graph caches will add to it. |
| Idle CPU (implied) | 0.13 s CPU over 20 s idle; 0.00 s over 5 s | Met — no repaint loop, no polling. |
| Warm status refresh < 150 ms | — | Not measurable until M1 (no status yet, no benchmark repo). |
| Log scroll 120 fps | — | Not measurable until M2. |
| 10 MB diffs lazy | — | Not measurable until M1. |

Supporting numbers:

- One git process costs 50–90 ms wall time (see G5). Opening a repository = `rev-parse` ×2 then
  three concurrent HEAD queries ≈ 200–250 ms until the overview is filled; the skeleton shows
  meanwhile.
- Memory history, same scenario: wgpu default memory hints 487 MB WS / 522 MB private →
  `MemoryHints::MemoryUsage` 163 / 224 MB. Debug builds sit higher (333 / 387 MB).
- Vulkan backend for comparison: 167 MB WS / 379 MB private, and no Mica.
- Low-power adapter (AMD integrated, DX12): one thread at 100 % of a core while idle (G4).
- Startup timings swing a lot with machine load (the same debug exe measured 634 ms quiet and
  3.0–4.9 s under a Unity shader compile). Compare like with like and note the load.

## Next steps (M1)

Scope: status, staging (file/hunk/line), text + image diffs, commit/amend, discard + undo — plus the
M1 row of "Where the remaining SPEC features land".

Suggested order:

1. `tools/` or `xtask` benchmark-repo generator (100k files, 100k commits, some LFS-like binaries);
   generate once outside the repo, record the path convention here.
2. `facet-git`: `status --porcelain=v2 -z --branch` parser (ordinary/renamed/unmerged/untracked/
   ignored records, branch headers incl. ahead/behind) + golden fixtures + integration tests
   (renames, unicode, spaces, conflicts, submodule entries, unborn branch).
3. FS watcher (`notify`) → 150 ms debounce → background status refresh with `.background()`;
   measure warm refresh on the benchmark repo against the 150 ms budget before building UI on it.
4. Diff engine: `git diff --no-ext-diff --no-color -z --patch` (index/worktree/HEAD variants) parsed
   into files → hunks → lines; lazy loading and virtualized rendering for huge diffs; syntect
   highlighting off the UI thread; word-level intra-line diff.
5. Staging: file (`add`/`restore --staged`), hunk and line via generated patches fed to
   `git apply --cached` (and `--reverse`) on stdin; integration tests for each, incl. CRLF files,
   no-newline-at-EOF, new/deleted files, binary files.
6. Binary-aware diff view (extension + git's binary detection → metadata card, never text) and
   image diff (side-by-side / swipe / onion-skin; `image` crate decode off-thread; tga/bmp/webp).
7. Commit / amend (message editor, hook output surfaced, never `--no-verify`).
8. Discard with undo: save what is being discarded first (stash-like object / patch in
   `.git/facet/`), then discard; undo toast restores it. Confirm before destructive actions.
9. Layout becomes sidebar · file list · diff with remembered splitters; every new control gets an
   `Action`, a palette entry and a tooltip showing its shortcut.
