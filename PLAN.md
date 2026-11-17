# Facet — Plan

Companion to `SPEC.md` (requirements + session protocol). This file is the working memory between
sessions: status, decisions, module map, gotchas, next steps. Keep it current.

## Milestone status

| # | Scope | Status |
| --- | --- | --- |
| M0 | Scaffold, docs, theme system, Mica window, repo open dialog, command palette shell | **done** (2026-11-17) |
| M1 | Status, staging (file/hunk/line), text + image diffs, commit/amend, discard + undo | **done** (2026-11-17) |
| M2 | Commit graph, branches, remotes, stash | **next** |
| M3 | Interactive rebase, conflicts, blame, search, reflog/undo | not started |
| M4 | LFS + locking, submodules, worktrees, settings, perf pass, MSIX + single-exe release | not started |

The project is a git repository; M0 was committed by the user as `fd06660` on `master`. Sessions
never commit — they suggest a commit message (commits are the user's call).

## Build, run, test

```
cargo build --workspace                                   # debug (deps optimized)
cargo run -p facet [-- <repo path>]                       # debug run, console attached
cargo test --workspace                                    # unit + integration + golden
cargo clippy --workspace --all-targets -- -D warnings     # must be clean
cargo fmt --all
cargo build --release -p facet                            # thin LTO; use this for all profiling
cargo build --profile dist -p facet                       # fat LTO, ~10.5 min; shipping only (M4)
cargo run --release -p facet-bench -- generate            # benchmark repo -> target/bench-repo (~65 s)
cargo run --release -p facet-bench -- measure --runs 10   # status / diff timings on it
```

`facet-bench generate [--files N] [--commits N] [--force] [DIR]` (defaults 100 000 / 100 000 /
`target/bench-repo`) builds the history with `git fast-import` and leaves a dirty tree (100 edits,
5 deletions, 50 untracked files, a 10 MB text file with every 7th line changed).
`facet-bench measure [--runs N] [--perf on|off] [DIR]` times status (first, warm foreground, warm
background), status parsing, the 10 MB diff load and diff parsing; `--perf` sets
`core.fsmonitor`/`core.untrackedCache` locally in the bench repo only.

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
| `FACET_PERF_LOG` | File to which status refresh / diff load / decorate timings are appended. |

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

## What M1 delivered

- Work view for every non-bare tab: repo header · file list (Staged / Changes sections, conflicts flagged,
  badges, split dir/name, skeleton while loading) · diff pane · commit box, with remembered splitters.
- Status: `status --porcelain=v2 -z --branch --untracked-files=all` parser (ordinary, renamed/copied,
  unmerged, untracked, ignored, submodule, branch headers incl. ahead/behind) + golden fixture.
- FS watcher (`notify`): 150 ms quiet / 1 s max debounce, `.git` noise filtered, batches classified
  as worktree / index / refs / overflow → targeted refresh (status always; the open diff only when
  its path, its rename source or the index changed).
- Text diff: `diff --no-ext-diff --no-color -z` parsed to hunks/lines; virtualized rows, sticky
  line-number gutter, syntect highlighting + word-level intra-line emphasis computed off the UI
  thread (20 k line budget), "line shortened" / "no newline at end of file" notes.
- Staging: file, all, hunk and line (click / drag / hunk-header click selects lines); hunk and line
  patches are generated (`patch.rs`) and applied with `git apply --cached` (unstage: `--reverse`).
- Binary-aware: extension table (.uasset/.umap/.fbx/.psd/.blend/…) + git's binary detection →
  metadata card with sizes and LFS-pointer detection; never a text diff.
- Image diff (png/jpg/tga/bmp/webp, decoded off-thread, capped at 4096 px textures): side by side,
  swipe, onion skin, checkerboard backdrop.
- Commit / amend: message editor (Ctrl+M, Ctrl+Enter), amend toggle prefills the last message; a
  failing commit (e.g. a rejecting hook) shows git's + the hook's output under the commit box, and
  every run's output is in the command log; hooks never bypassed.
- Discard (file / hunk / lines) with confirm dialog → content moved to `.git/facet/undo/<id>/` first
  → undo toast + Ctrl+Z; store pruned to 50 entries / 7 days.
- Per-repo opt-in offer for `core.fsmonitor` + `core.untrackedCache` after two consecutive refreshes
  over 150 ms (written with `config --local` only after the user accepts).
- `facet-bench` crate: benchmark repo generator + measurement tool.
- 161 tests: facet 67 unit; facet-bench 2; facet-git 36 unit + 56 integration/golden.

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
15. **Foreground vs background status.** Watcher-triggered refreshes run `.background()`
    (`GIT_OPTIONAL_LOCKS=0`, never takes `index.lock`); refreshes after Facet's own operations and
    F5 run in the foreground so the index stat cache is refreshed. Every request carries a per-tab
    **epoch**; results older than the newest request are dropped.
16. **Facet ignores its own index writes.** After an operation the app records `.git/index`'s
    `(mtime, len)` stamp; a watcher batch whose only news is that same stamp does not trigger
    another refresh (no refresh loops).
17. **Targeted diff reload (`WorkState::diff_stale`).** A status refresh never reloads the open diff
    by itself; it is marked stale by a foreground request, a watcher batch touching its path / rename
    source, or an index change. An identical reload keeps selection, colours and scroll.
18. **Partial staging = generated patch + `git apply --cached`** on stdin (`--reverse` to unstage,
    `git apply --reverse` on the worktree for partial discard). Partial staging is disabled for
    renames, conflicts, submodules and binaries (file-level actions only), where a hand-built patch
    could not be trusted.
19. **Undo by moving, not by stash.** Discard copies the current content (or the reverse patch) into
    `.git/facet/undo/<id>/` with a manifest *before* touching the working tree; undo restores it.
    It works for untracked files (stash would need `-u` and touches everything) and is invisible to
    other tools. Destructive actions go through `ui/confirm.rs` and always end with an undo toast.
20. **Decorations are async and appearance-gated.** Syntax/emphasis spans are computed in
    `spawn_blocking` with a cancellation token per diff; they are drawn only if computed for the
    current dark/light palette, otherwise the plain text shows until the recompute lands.
21. **Clipboard** goes through egui's `ctx.copy_text` (eframe's `clipboard` feature, arboard
    underneath) — no direct `arboard` dependency.
22. **Benchmarks live in their own crate** (`facet-bench`) and build history with `git fast-import`
    (100k commits in ~40 s; per-commit `git commit` would take hours, G5).

## Module map

```
Cargo.toml                    workspace, shared deps, lints (clippy::all = deny), profiles
crates/facet-git/
  src/lib.rs                  public API re-exports
  src/runner.rs               Git, GitCommand (builder), GitOutput; spawn/stream/cancel
  src/cmdlog.rs               CommandLog, CommandRecord, CommandOutcome
  src/error.rs                GitError
  src/repo.rs                 RepoInfo, Head, version(), discover(), head(), commit_summary()
  src/status.rs               status() (fg/bg), PerfConfig, perf_config(), set_perf_config()
  src/diff.rs                 DIFF_ARGS, diff_file(), read_blob(), object_sizes(), LFS pointers
  src/patch.rs                build_patch(): hunk/line selection -> applyable patch
  src/stage.rs                stage/unstage paths/all, apply_patch() (git apply --cached)
  src/commit.rs               commit()/amend with hook output, last_commit_message()
  src/discard.rs              discard_files(), discard_patch(), undo store, list/prune_undo()
  src/parse/mod.rs            shared helpers (utf8)
  src/parse/commit.rs         COMMIT_SUMMARY_FORMAT, CommitSummary, parse_commit_summary
  src/parse/status.rs         porcelain v2 -z parser
  src/parse/diff.rs           numstat + patch parser (files -> hunks -> lines)
  src/parse/version.rs        GitVersion, parse_version
  tests/common/mod.rs         TestRepo: isolated temp repo/bare repo, fixed identity + dates
  tests/repo.rs               discovery + HEAD integration tests
  tests/runner.rs             runner: streaming, stdin, cancel, logging, env
  tests/status.rs             status: renames, unicode, spaces, conflicts, submodules, unborn
  tests/staging.rs            file/hunk/line stage + unstage, CRLF, no-EOL, new/deleted, binary
  tests/commit.rs             commit, amend, hooks (output surfaced, failure reported)
  tests/discard.rs            discard file/hunk/lines + undo, untracked files, pruning
  tests/golden.rs + golden/   parser golden fixtures (version, commit, status_v2, diff)
crates/facet-bench/
  src/main.rs                 generate (fast-import benchmark repo) + measure (status/diff timings)
crates/facet/
  build.rs                    embeds the app manifest (PerMonitorV2, long paths, UTF-8)
  src/main.rs                 runtime, wgpu/DX12 setup, viewport, eframe launch
  src/app.rs                  FacetApp state, Event, RepoTab, action dispatch, persistence
  src/app_work.rs             status/diff/stage/commit/discard/undo requests and their events
  src/workspace.rs            per-tab WorkState: status, selection, open diff, line selection
  src/loader.rs               off-thread diff/image/blob loading, image decode limits
  src/watcher.rs              notify watcher, classify + debounce -> FsChange, index stamp
  src/highlight.rs            syntect highlighting (off-thread), span model
  src/textdiff.rs             word-level intra-line diff
  src/filekind.rs             extension -> Text / Image / Binary(name)
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
  src/ui/content.rs           welcome screen, bare-repo overview, loading skeleton
  src/ui/changes.rs           work view layout, repo header, file list, perf offer, commit box
  src/ui/diff.rs              diff pane: header actions, virtualized text diff, gutter, selection
  src/ui/image_diff.rs        side by side / swipe / onion skin, checkerboard
  src/ui/confirm.rs           modal confirm dialog for destructive actions
  src/ui/command_log.rs       command log panel
  src/ui/palette.rs           command palette
  src/ui/toast.rs             toasts
```

## Where the remaining SPEC features land

Features in SPEC §2–§4 that the milestone one-liners do not name explicitly:

| Milestone | Also includes |
| --- | --- |
| M1 (done) | FS watcher (`notify`, ~150 ms debounce → targeted refresh); syntax highlighting (`syntect`) + word-level intra-line diff; binary-aware handling (.uasset/.umap/.fbx/.psd/.blend → metadata, never text diff); lazy 10 MB diffs; clipboard via `arboard`; per-repo opt-in offer for `core.fsmonitor` / `core.untrackedCache`; **benchmark repo generator** (100k files / 100k commits) and the first real perf measurements (status refresh). |
| M2 | Tags; multiple remotes; fetch / pull (rebase or ff-only) / push (force-with-lease); credentials (askpass / GCM — see G7); process-tree kill via Job Objects (G6); partial stash; virtualized graph at 120 fps on the benchmark repo; Windows toast notifications for long operations; GitHub/GitLab/Azure DevOps link-outs. |
| M3 | Cherry-pick, revert, reset; file history with rename following; search (message/author/file/pickaxe); per-extension external diff/merge tools (UnityYAMLMerge, P4Merge, Beyond Compare); auto-stash + reflog-based undo for destructive operations. |
| M4 | Unity/Unreal `.gitignore` templates; open in Windows Terminal / VS Code / Rider; settings UI (theme, fonts, external tools, git path); sparse-checkout and partial-clone verification pass; embedded `.ico` + version resource; single-instance handling. |

## Gotchas (read before changing related code)

- **G1 — eframe/egui 0.37 API.** `App` has `logic(ctx, frame)` + `ui(ui, frame)` (no `update`).
  `Id::new` is deprecated → `Id::unique`; `ui.id()` → `ui.scope_id()`; `DroppedFile::path()` is a
  method; `TextEdit::frame` takes a `Frame`; variable-font axes go through `FontTweak::coords`.
  Do not blanket-replace `::new(` — `FontId::new` and `LayerId::new` are still correct.
  Learned in M1: `LayoutSection.byte_range` is `Range<ByteIndex>` (slice with `.start.0..end.0`);
  with `TextEdit::frame(Frame::NONE)` the `.margin()` is ignored — use
  `Frame::NONE.inner_margin(..)`; in `ScrollArea::show_viewport` `ui.max_rect().min` is the content
  origin; repeating textures need `TextureOptions { wrap_mode: TextureWrapMode::Repeat, .. }`.
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
  Shell `sed` substitutions mangle escapes too: in M1 a sed edit turned the BOM char literal
  (backslash-u-{feff}) into `'{feff}'`. Anything containing a backslash goes through Edit/Write.
  Stop a running `facet.exe` before `cargo build --release` (the exe is locked).
- **G13 — `git log` on an unborn branch fails by design**; `head()` ignores that failure. Any new
  caller must treat "no commits yet" as a state, not an error.
- **G14 — session-restored tabs must not reorder the recent list** (`RepoTab::restored`); only
  user-initiated opens call `touch_recent`.
- **G15 — settings JSON may start with a UTF-8 BOM** (Notepad, PowerShell 5.1 `Set-Content`/
  `Out-File`). The loader strips it; without that the file was treated as corrupt and renamed.
  Write test settings with `[IO.File]::WriteAllText` (no BOM).
- **G16 — `core.fsmonitor` is not a free win.** On the dev machine (NVMe) warm status on the
  100k-file bench repo was ~111 ms without it and ~146 ms with fsmonitor + untrackedCache. The
  offer is shown only when measured refreshes exceed 150 ms, and is never applied without consent;
  re-measure on slower disks before making it more prominent.
- **G17 — never reload the diff on every status refresh.** A 10 MB diff costs ~260 ms and ~11 MB
  per load; use `diff_stale` (decision 17). Facet's own index writes must not retrigger refreshes
  (decision 16).

## Known issues / deliberate limits after M1

- True cold start (first launch after a reboot) has not been measured; only warm-cache launches.
- Frame time while scrolling the 10 MB diff was not measured (no number, only an impression).
- Light theme of the M1 views, onion-skin mode and drag-to-select lines were not visually verified
  in the app (logic is unit-tested; dark theme, side by side, swipe and click-select were checked).
- No keyboard scrolling in the diff (PgUp/PgDn/Home/End do nothing); arrow keys move between files.
- The sidebar "Recent" list is empty when all tabs were session-restored (by design, G14).
- No single-instance guard: two Facet windows share one `settings.json`; last writer wins.
- Maximized window + auto-hide taskbar: the taskbar may not slide in (needs the
  `ABM_GETAUTOHIDEBAR` one-pixel inset in `WM_NCCALCSIZE`).
- Many open tabs are squeezed, not scrolled; the palette lists all tabs.
- The exe has no embedded `.ico`/version resource (M4).
- Bare repositories show the overview only (no work view: there is no working tree).
- Folder picker and drag-and-drop were never exercised end-to-end by automation (M0 note); still on
  the manual checklist.
- Partial (hunk/line) staging is unavailable for renamed, conflicted, submodule and binary entries;
  conflicts can only be marked resolved (full conflict UI is M3).
- Diffs beyond 20 000 lines show without syntax/word highlighting (plain text, still virtualized).
- G6 still applies: cancelling kills `git.exe` but not hook children; commit hooks run to completion.

## Measurements

Dev machine: Windows 11 Home 26200, AMD integrated GPU + NVIDIA GT 1030, Git for Windows.
Method: `FACET_STARTUP_LOG` for time to first presented frame; `Get-Process` CPU time / working
set / private bytes sampled over a fixed idle window; one clean instance at a time.

Benchmark repo (`facet-bench generate`): 100 000 commits, 100 001 files, 10 MB text file; generated
in 65 s (fast-import 41.6 s, checkout 23.1 s). Dirty tree as described under Build.

M1 `release` build, measured on the bench repo:

| Budget (SPEC §2.4) | Measured | Verdict |
| --- | --- | --- |
| Cold start < 1 s | 901 / 903 ms to first frame with the bench repo restored (warm cache, GT 1030). M0 shell: 842 / 887 ms. | Within budget warm. **True cold start not measured.** |
| Warm status refresh < 150 ms | `facet-bench measure`: first 254 ms; warm foreground median 111 ms (108–118); warm background median 112 ms; parse < 1 ms (19 KB). In-app: first 141–143 ms, warm 118–126 ms. | **Met** (no fsmonitor). With fsmonitor + untrackedCache: ~146 ms (slower, G16). |
| 10 MB diffs lazy | Load median 256 ms (11.6 MB, 159 997 lines; parse 6 ms) via `facet-bench`; in-app 255–296 ms off the UI thread, decorate 6–7 ms (beyond line budget, so plain). The UI keeps painting meanwhile (skeleton). | **Met**: UI thread never blocks; rows virtualized. Scroll frame time not measured. |
| Idle RAM < 300 MB | File list only: 166 MB working set / 224 MB private. 10 MB diff open, 20 s idle: 188 MB / 253 MB. | Met. |
| Idle CPU | 0.03 s CPU over 20 s idle with the 10 MB diff open. | Met: watcher is event-driven, no polling. |
| Log scroll 120 fps | — | Not measurable until M2 (graph). |

M0 numbers (same machine, for comparison): 842 / 887 ms first frame, 163 MB WS / 224 MB private,
0.13 s CPU over 20 s idle.

Supporting numbers:

- One git process costs 50–90 ms wall time (see G5). Opening a repository = `rev-parse` x2 then
  concurrent HEAD/status queries; the skeleton shows meanwhile.
- Memory history: wgpu default memory hints 487 MB WS / 522 MB private -> `MemoryHints::MemoryUsage`
  163 / 224 MB. Debug builds sit higher (333 / 387 MB).
- Vulkan backend for comparison: 167 MB WS / 379 MB private, and no Mica.
- Low-power adapter (AMD integrated, DX12): one thread at 100 % of a core while idle (G4).
- Startup timings swing a lot with machine load. Compare like with like and note the load.

## Next steps (M2)

Scope: commit graph, branches, remotes, stash — plus the M2 row of "Where the remaining SPEC
features land" (tags, multiple remotes, fetch/pull/push, credentials, Job Objects, partial stash,
120 fps graph on the bench repo, Windows toasts, link-outs).

Suggested order:

1. Job Objects first (G6): spawn network/hook-running git inside a job with `KILL_ON_JOB_CLOSE`;
   integration test that cancelling kills a child process.
2. `facet-git`: `log` with an explicit `--format` + `-z` (hash, parents, author, dates, subject),
   refs from `for-each-ref --format`, streamed in pages; graph lane assignment as a pure, tested
   function; golden fixtures.
3. Virtualized graph view; measure 120 fps scrolling over the 100k-commit bench repo (record frame
   times, not impressions); commit detail reuses the M1 diff pane.
4. Branches and tags: list, create, rename, delete (confirm + undo via the old SHA), checkout/switch
   with dirty-tree handling, upstream tracking.
5. Remotes: list/add/remove; fetch, pull (rebase or ff-only), push (`--force-with-lease` only) with
   progress; credentials via GCM's own UI plus a `GIT_ASKPASS`/`SSH_ASKPASS` helper (G7); never
   write global config (G9).
6. Stash: list, push (incl. partial via pathspec / `--staged`), apply, pop, drop (confirm + undo).
7. Windows toast notifications for long operations; GitHub/GitLab/Azure DevOps link-outs from the
   remote URL.
8. Every new control: `Action` + palette entry + shortcut tooltip; update PLAN.md measurements.
