# Facet — Plan

Companion to `SPEC.md` (requirements + session protocol). This file is the working memory between
sessions: status, decisions, module map, gotchas, next steps. Keep it current.

## Milestone status

| # | Scope | Status |
| --- | --- | --- |
| M0 | Scaffold, docs, theme system, Mica window, repo open dialog, command palette shell | **done** (2026-11-17) |
| M1 | Status, staging (file/hunk/line), text + image diffs, commit/amend, discard + undo | **done** (2026-11-17) |
| M2 | Commit graph, branches, remotes, stash | **done** (2026-11-18) |
| M3 | Interactive rebase, conflicts, blame, search, reflog/undo | **done** (2026-11-18) |
| M4 | LFS + locking, submodules, worktrees, settings, perf pass, MSIX + single-exe release | **done** (2026-11-18) |
| M4.1 | Release hardening: install + sign the MSIX and verify it, true cold start, the unclicked flows, remaining light-theme dialogs, M4 known issues | **next** (defined in PLAN, not in SPEC) |

M4 is the last SPEC milestone; every SPEC §4 feature is shipped. M4.1 exists because some of
the release path could not be verified inside a session (see "Next steps (M4.1)").

The project is a git repository; the user committed M0 as `fd06660` and M1 as `15dffdd` on `master`;
M2, M3, M4 and the UI redesign (title bar switcher, Settings page, Add repository) went in
together as the next commit, made at the user's request. Sessions only commit when the user asks —
otherwise they suggest a commit message (commits are the user's call).

## Build, run, test

```
cargo build --workspace                                   # debug (deps optimized)
cargo run -p facet [-- <repo path>]                       # debug run, console attached
cargo test --workspace                                    # unit + integration + golden
cargo clippy --workspace --all-targets -- -D warnings     # must be clean
cargo fmt --all
cargo build --release -p facet                            # thin LTO; use this for all profiling
cargo build --profile dist -p facet                       # fat LTO, ~5–10 min; the shipping single exe
cargo run --release -p facet-bench -- package             # MSIX from target/dist/facet.exe -> target/msix
cargo run --release -p facet-bench -- generate            # benchmark repo -> target/bench-repo (~65 s)
cargo run --release -p facet-bench -- measure --runs 10   # status / diff timings on it
cargo run --release -p facet-bench -- graph               # branchy 100k-commit repo -> target/bench-graph
cargo run --release -p facet-bench -- history --runs 3    # reflog / search / file history / blame timings
```

`facet-bench history [--runs N] [DIR]` (default `target/bench-repo`) times the reflog, the four
search kinds (first page of 200 and all results), `--follow` file history of a source file, and
blame (first lines and all) of that file and of the 10 MB `Docs/Generated/Big.txt`.

`facet-bench graph [--commits N] [--branches N] [--force] [DIR]` (defaults 100 000 / 24 /
`target/bench-graph`) builds a history with 24 long-lived topic branches merged back into `main`
repeatedly (~6 000 merges, 25 graph lanes) — the graph stress case; `bench-repo` is linear.

`facet-bench package [--exe PATH] [--publisher CN=…] [--makeappx PATH] [DIR]` (defaults
`target/dist/facet.exe`, `CN=Facet`, the newest Windows SDK `makeappx.exe`, `target/msix`) writes
`DIR/layout` (exe, `AppxManifest.xml`, logos drawn by `icon_art.rs`) and packs
`DIR/Facet_<version>_x64.msix`, **unsigned**. To install: sign it with a certificate whose
subject equals `--publisher` (`signtool sign /fd SHA256 /f <pfx> <msix>`), or enable Developer
Mode and `Add-AppxPackage -Register target\msix\layout\AppxManifest.xml`.

`facet-bench generate [--files N] [--commits N] [--force] [DIR]` (defaults 100 000 / 100 000 /
`target/bench-repo`) builds the history with `git fast-import` and leaves a dirty tree (100 edits,
5 deletions, 50 untracked files, a 10 MB text file with every 7th line changed).
`facet-bench measure [--runs N] [--perf on|off] [DIR]` times status (first, warm foreground, warm
background), status parsing, the 10 MB diff load and diff parsing; `--perf` sets
`core.fsmonitor`/`core.untrackedCache` locally in the bench repo only.

Requirements: Rust stable (built with 1.97.1), Git for Windows on `PATH` (tested with 2.45.1),
Windows 11. Build times on the dev machine: debug incremental ~25 s, `release` ~3.5–4 min, `dist`
~5.3 min (M4; was ~10.5 min in M1). The MSIX step additionally needs the Windows 10/11 SDK
(`makeappx.exe`); the exe itself needs only the MSVC toolchain (`build.rs` writes the `.res`).
Tests spawn the real `git` in temp directories with `GIT_CONFIG_GLOBAL` pointed at an empty file and
`GIT_CONFIG_NOSYSTEM=1`, so they never read or write the user's config.

Diagnostic environment variables:

| Variable | Effect |
| --- | --- |
| `FACET_CONFIG_DIR` | Use this folder instead of `%APPDATA%\Facet` for `settings.json` (isolates test runs). |
| `FACET_STARTUP_LOG` | File to which `first frame presented after N ms on <GPU adapter>` is written. |
| `WGPU_POWER_PREF` | `low` / `high` — override GPU adapter choice (see gotcha G4). |
| `WGPU_BACKEND` | `dx12` (default) / `vulkan`. Vulkan works but loses Mica (no alpha swapchain). |
| `FACET_PERF_LOG` | File to which status refresh / diff load / decorate / history load timings are appended. |
| `FACET_BENCH_SCROLL` | `1`: once the active tab's history has loaded, switch to History and scroll it 37 px per frame for 120 warm-up + 1 500 measured frames, then write per-frame CPU percentiles (p50/p90/p99/max, frames over 8.33 ms) and frame-interval percentiles to `FACET_PERF_LOG` and a toast. |
| `FACET_BENCH_SCROLL=diff:<path>` | Same, but open the changes view, select that changed file and scroll its text diff (20 px rows); used for the 10 MB diff (`diff:Docs/Generated/Big.txt`). |
| `FACET_BENCH_SEARCH` | `<message\|author\|file\|code\|regex>:<text>`: once the active tab's history has loaded, run that search in History (timings go to `FACET_PERF_LOG`); combined with `FACET_BENCH_SCROLL` the scroll benchmark then runs over the results. |

## Install and update on the owner's PC

The owner uses Facet installed from this folder. **Double-click `update.cmd`** (repo root) to
install or update it: it runs `scripts/build-installer.ps1 -Install`, which builds the `dist`
exe, builds `crates/facet-setup` with that exe embedded (`FACET_SETUP_PAYLOAD`), writes
`target\installer\Facet-Setup-<version>.exe` and runs it. `-Quiet` runs the setup with no dialogs.

`facet-setup` (per-user, no admin, no certificate — unlike the MSIX):

* installs to `%LOCALAPPDATA%\Programs\Facet` (`facet.exe` + `uninstall.exe`, a copy of the
  setup), a Start menu shortcut, and a Settings → Apps entry under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Facet`;
* updates in place, also while Facet is open: the running `facet.exe` is renamed to
  `facet-<nanos>.old` (Windows allows renaming a running exe, not deleting it) and the new one
  written; the open window keeps the old build until reopened (the single-instance pipe would
  otherwise hand a new launch to it), and the next setup run deletes the `.old` files;
* uninstall: `uninstall.exe --uninstall [--quiet]` refuses while Facet is open, removes the
  shortcut, the registry entry and the folder (a detached `cmd` removes the folder after the
  uninstaller exits); `%APPDATA%\Facet\settings.json` is kept.

Builds are stamped with `FACET_BUILD_ID` = short commit hash (`-modified` if the tree is dirty),
shown in the Settings footer (`Facet 0.1.0 (c323554)`) and in Settings → Apps, so the owner
can tell which build is installed. Without the env var (everyday builds) nothing is shown, and
`facet-setup` gets an empty payload and refuses to run.

**Session rule:** after a session changes the app, its closing summary tells the owner to run
`update.cmd` to get the change onto their PC. Bump `[workspace.package] version` when a
milestone ships.

## What M0 delivered

- Cargo workspace: `facet-git` (git plumbing library) + `facet` (the app).
- Window: custom 48 px title bar with **real DWM caption buttons** (snap layouts, hover states),
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

## What M2 delivered

- **Job Objects (G6 resolved):** every git process is assigned to a job with
  `KILL_ON_JOB_CLOSE`; cancel terminates the whole tree (hooks, `ssh`, `git-remote-https`,
  credential helpers). Integration tests: cancel and drop kill git's child; detached children of
  a command that finished normally survive.
- **History view** (Changes / History segment in the repo header, Ctrl+1 / Ctrl+2): `git log
  --date-order -z` streamed in pages (first 200 commits, then 5 000 per page), lanes assigned
  incrementally on the worker (`graph.rs`, pure + tested), virtualized rows with adaptive lane width
  (14 → 8 px, then clamped), merge and HEAD rings, ref badges (branches, remote branches, tags,
  HEAD). Reloads when ref tips change. Commit detail pane: full message, author/committer, parents,
  changed files (`diff-tree -M --raw --numstat`), per-file diff in the M1 diff renderer.
- **Sidebar** rewritten as one virtualized list: Branches (ahead/behind), Remotes → remote
  branches, Tags, Stashes, Recent; collapsible sections with a hover "+" button; click reveals the
  commit in History, double-click switches/checks out; context menu on every row.
- **Branches & tags:** create (at HEAD or any commit), rename, delete (confirm → undo restores the
  old tip), switch (a refused switch over a dirty tree offers **Stash and switch**), check out
  detached, check out a remote branch as a tracking branch (upstream also set by publish);
  lightweight or annotated (message → `tag -a --file=-`) tags, delete with undo, push tag.
- **Remotes:** add / remove (undo restores URL + push URL), fetch (one or `--all`; pruning follows
  the repo's own `fetch.prune`), pull
  (`--ff-only` default or `--rebase`, setting + palette), push / publish (`--set-upstream`), force
  push **only** as `--force-with-lease=<ref>:<oid seen in the last refs snapshot>`; live progress
  line and Cancel in the header; one network op per tab.
- **Credentials (G7 resolved):** Facet's own exe is `GIT_ASKPASS`/`SSH_ASKPASS` (`facet.exe` run
  with the askpass env forwards the prompt over a local named pipe with a random per-session
  token that only git's children inherit, and prints the answer).
  Prompts appear in Facet's prompt dialog (password fields masked). GCM still shows its own UI;
  `GCM_MODAL_PARENTHWND` points it at Facet's window. `GIT_TERMINAL_PROMPT=0` stays.
- **Stash:** list, push (all / include untracked / message), **partial** stash of the
  selected files (`:(literal)` pathspecs) and staged-only (`--staged`), apply, pop, drop (confirm →
  undo via `stash store` of the dropped commit).
- **Toasts:** network ops ≥ 5 s that finish while Facet is not focused raise a Windows
  notification (`Shell_NotifyIconW` balloon → toast on Windows 11). In-app toasts as before.
- **Link-outs:** GitHub / GitLab / Azure DevOps (https + ssh URL forms) — open commit, branch,
  tag, "new pull request" from context menus and the palette.
- `facet-bench graph` (branchy repo) and `FACET_BENCH_SCROLL` (in-app frame-time benchmark).
- Every control above has an `Action` + palette entry; new shortcuts Ctrl+1/2, Ctrl+Shift+F
  (fetch), +L (pull), +P (push), +N (new branch), +S (stash).
- 216 tests: facet 78 unit + 2 integration (askpass end-to-end against a local HTTP server that
  demands Basic auth); facet-bench 2; facet-git 56 unit + 78 integration/golden.

## What M3 delivered

- **Operation state:** the rebase / merge / cherry-pick / revert / am / bisect in progress is read
  from the git directory (`operation.rs`, `rebase-merge`, `rebase-apply`, `MERGE_HEAD`,
  `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_LOG`) with every refs snapshot. A warning banner under
  the repo header shows step n/m, the branch, the stopped commit and the conflict count, with
  Continue (disabled while conflicts remain), Skip, Abort… (confirm). Palette entries for all.
- **Cherry-pick, revert, reset (soft / mixed / hard behind a confirm), rebase onto** from the
  History row menu and the palette. Local changes are **auto-stashed** around cherry-pick, revert
  and rebase and re-applied after. Every finished rewrite pushes an `UndoPoint` (HEAD before +
  reset mode + auto-stash) → "Undo" toast and **Ctrl+Shift+Z** (Ctrl+Z stays the discard undo).
  A hard reset is undone with `reset --keep` so it never clobbers newer local edits.
- **Merge into the current branch** (SPEC §4.2; M2 had left it out): sidebar branch and remote
  branch menus, History row menu, palette ("Merge X into main"). `merge --no-edit` with git's
  own message and `merge.ff`; auto-stash, undo and the conflict flow like cherry-pick.
- **Interactive rebase:** modal todo editor (pick / reword / edit / squash / fixup / drop as
  chips or the keys P R E S F D; reorder by dragging a row's handle, with Alt+Up/Down or with the
  row arrows; reword message edited
  inline; Ctrl+Enter starts). Facet's own exe is `GIT_SEQUENCE_EDITOR` and `GIT_EDITOR`
  (`--facet-sequence-editor` / `--facet-message-editor`, state in `<git dir>/facet-rebase/`), so
  an `edit` stop rewords correctly after Continue. Hooks run as usual (G8).
- **Conflicts:** per-file 3-way resolver: each chunk shows Ours · Base · Theirs side by side.
  The base comes from the markers (diff3 / zdiff3 style) or, with git's default style, from
  re-merging the index stages with `git merge-file -p --zdiff3` into temp files, copied only onto
  chunks whose ours / theirs text matches (`conflict::fill_bases`; never writes the file or index).
  Common text is collapsed; per chunk: Take Ours / Take Theirs / Both (either order); All Ours /
  All Theirs; Whole File Ours / Theirs. Save and Mark Resolved once every chunk has a choice.
  Take Ours / Theirs also from the file list, the diff header and the palette (handles
  deleted-by-us/them); **external merge tool** per extension.
- **External diff / merge tools per extension** (`settings.external_tools`: extensions, diff
  command, merge command with `$LOCAL $REMOTE $BASE $MERGED` placeholders — UnityYAMLMerge, P4Merge,
  Beyond Compare all fit). Added from the palette ("Add External Diff / Merge Tool…"), removed from
  the palette. Run in a job object with temp files under `<git dir>/facet-tools/`; nothing is
  written to git config (G9). A merge tool that exits 0 and leaves no markers stages the file.
- **Search** (Ctrl+F) above the commit list: Message, Author, File (path substring), Code
  (`-S` pickaxe), Diff Regex (`-G`); results stream into the History list (graph hidden) in the same
  200 / 5 000 pages as the log; Escape returns to the full history (kept aside, not reloaded —
  except histories over 20 000 commits, which are freed and re-read; decision 35).
- **File history** (`log --follow --name-status`, rename-following; the row shows the file's old
  name as a pill and the detail opens that file) from the file lists, commit detail, palette,
  Ctrl+Shift+H.
- **Blame** (Ctrl+Shift+B, file lists, commit detail "Blame at This Commit"): `blame
  --incremental` streamed into a virtualized view with an oid / author / age gutter at the start
  of each run; double-click jumps to the commit in History; "File History" button in the header.
- **Reflog** (Ctrl+Shift+R): HEAD's reflog as a History list with each step's message as a pill;
  any entry can be checked out, reset to, cherry-picked — the general "get it back" path.
- `facet-bench history` and `FACET_BENCH_SEARCH`.
- 267 tests: facet 87 unit + 2 integration; facet-bench 2; facet-git 79 unit + 97
  integration/golden (new: `tests/operations.rs` 16 end-to-end operation tests; golden fixtures
  `blame`, `file_log`, `reflog`).

## What M4 delivered

- **Compact oids** (`facet_git::Oid`, `oid.rs`): history, search results, the reflog and the graph
  builder hold ids as inline bytes (`Copy`, no heap) with parents as `Parents` (none / one inline / boxed slice for merges),
  hex produced on demand (`Hex`). Idle memory with 100k commits dropped from 296–298 MB to
  ~264 MB private (decision 37).
- **Git LFS:** `lfs_version` detection, `lfs_usage` (is LFS configured / are there LFS
  attributes) with every refs snapshot; which paths LFS stores and which are `lockable` comes
  from `git check-attr -z filter lockable` over the status paths (git-lfs has no JSON
  `ls-files`, G30). File rows show "LFS" / lock badges with tooltips (locked by you, locked by
  someone else, lockable but unlocked). **Locks:** `lfs locks --json` (list, also `--verify` for
  ours), lock, unlock, force unlock behind a confirm; a **Locks** sidebar section; palette
  entries per file. **Track:** "LFS: Track Files…" (pattern, optional `--lockable`) writes
  `.gitattributes` via `git lfs track`. **Transfers:** fetch / pull / push set
  `GIT_LFS_FORCE_PROGRESS=1` so LFS's own progress lines reach the progress bar (G31); "LFS:
  Download Files (Pull)". Lock commands run on the credential git (askpass / GCM, G7).
- **Submodules:** list from `.gitmodules` + gitlinks in the index (`git config -z --file
  .gitmodules`, `ls-files -z --stage`; golden fixtures), a **Submodules** sidebar section,
  update (`--init --recursive`, one or all), sync URLs, open a checked-out submodule as a tab.
  A submodule pointer change in the diff pane shows the commits it brings in and drops
  (`submodule_range`, `log` in the submodule's checkout).
- **Worktrees:** `worktree list --porcelain -z` (golden fixture) in every refs snapshot, a
  **Worktrees** sidebar section, add (existing or new branch, folder suggested next to the
  repository), open as a tab, remove (never forced; confirm; **undo re-adds it** on its branch or
  commit), prune. Switching to, or adding a worktree for, a branch checked out in another
  worktree is refused with a message naming that folder.
- **Settings page** (sidebar Settings item, **Ctrl+,**, palette "Settings…"; a page in the
  content area, cards of rows): theme (System / Light / Dark previews), interface
  size (90–150 %, kept in sync with egui's Ctrl+= / Ctrl+- zoom), code font (Cascadia Mono /
  Cascadia Code / Consolas; fonts rebuilt off-thread), git executable (typed or Browse…, checked
  with `git version` before it is used; "Use Git from PATH"), pull mode, external diff / merge
  tools (list, remove, add, **presets for UnityYAMLMerge / P4Merge / Beyond Compare** with the
  detected executable filled in), Open In, `.gitignore` templates. Every setting is also an
  `Action` with a palette entry.
- **Open in** Windows Terminal (`wt -d`), VS Code, Rider and Explorer: an Open In button in the
  title bar for the active repository plus palette entries. Installs are detected in the
  background at startup (well-known folders and `PATH`, app execution aliases, the JetBrains
  Toolbox script, Unity Hub editors for UnityYAMLMerge); only detected programs are offered.
- **`.gitignore` templates** (Unity, Unreal Engine): preview of exactly the lines that will be
  appended (rules the file already has are skipped), append keeps the file's line endings, and an
  **Undo** toast restores the previous file (or removes a file Facet created) unless it was edited
  since.
- **Single instance** (`instance.rs`): the first Facet owns a named pipe keyed by its settings
  folder; a second launch (e.g. `facet D:\repo` from a terminal) hands its absolute path over
  and exits, and the first opens it as a tab and comes to the front (G33).
- **Sparse checkout and partial clone** verified by integration tests: a `--filter=blob:none`
  clone runs status, log (with the graph) and the refs snapshot without fetching a single blob
  (missing-object count unchanged); a cone-mode sparse checkout shows only the cone (no phantom
  deletions), and stage all + commit keep the files outside the cone in the commit.
- **Executable resources:** `build.rs` writes a `.res` by hand (`build/res.rs`; no `rc.exe`):
  an icon group (16–64 px DIBs + a 256 px PNG) drawn by the same code as the window icon
  (`icon_art.rs`), and a `VERSIONINFO` from the crate version. `tests/resources.rs` reads both
  back through Win32.
- **Release:** `cargo build --profile dist -p facet` is the single self-contained exe (no
  runtime files beyond system fonts and Git for Windows). `facet-bench package` lays out and packs
  an **unsigned MSIX** (`AppxManifest.xml`, logos rendered from the icon code, execution alias
  `facet.exe`, `runFullTrust`) with the newest Windows SDK `makeappx.exe`; signing is the owner's
  call (decision 41).

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
23. **History = `git log --date-order` streamed, lanes computed on the worker.** `--date-order`
    guarantees children before parents (needed for incremental lane assignment) at the cost of
    first output (~470 ms vs ~95 ms in default order on 100k commits). `GraphBuilder` is fed page
    by page off the UI thread; the UI only draws `GraphRow`s. The log is reloaded when any ref tip
    changes (refs snapshot compare), never on plain status refreshes.
24. **Refs = one snapshot of three concurrent commands** (`for-each-ref`, remotes via
    `config -z --get-regexp`, `stash list -z`), requested after every op and on ref-touching
    watcher batches. Sidebar, badges, header buttons and force-with-lease all read that snapshot.
25. **Repository ops go through `repo_op(label, closure)`**: takes the tab's ops lock, counts as
    busy, returns an `Outcome` (`Done` / `Undo{title, action}` / `SwitchRefused`), and always ends
    with a foreground status + refs refresh. Destructive ops return an undo action that restores
    exactly what was removed (branch tip, tag object, remote URLs, dropped stash commit).
26. **Network ops are separate from repo ops**: one per tab (`NetworkJob` Fetch / Pull / Push /
    PushTag), cancellable, git's `--progress` line shown verbatim (never parsed). Only Pull also
    takes the ops lock (it changes the working tree). They run on a `Git` built with
    `credential_git` (askpass + GCM env); local commands do not get those variables.
27. **Askpass via Facet's own exe** (`askpass::helper_main` runs before eframe when the askpass
    env is present), so there is no second binary to ship. The prompt dialog (`ui/prompt.rs`)
    serves both naming prompts and credential prompts.
28. **Graph lanes are adaptive**: the graph gets at most 40 % of the list width; lanes shrink
    from 14 to 8 px before overflow lanes are clamped to the last column (dense histories stay
    readable instead of piling onto one column).
29. **Force push exists only as `--force-with-lease=<ref>:<oid>`** with the oid from the refs
    snapshot the person was looking at, behind a confirm dialog. Plain `--force` is never built.
30. **Operation state is read from files, not from git commands** (`facet_git::operation(git_dir)`
    in `spawn_blocking`, joined with the refs snapshot): no extra process per refresh (G5). The
    banner, the palette and "may I rewrite history now?" checks all read `power.operation`.
31. **History rewrites return `Outcome::Rewrote`** (title, `Progress` Finished / Stopped, optional
    `UndoPoint`, note). Finished → undo toast + undo stack; Stopped → the operation banner takes
    over (conflicts, `edit` stops). All of them go through `repo_op_info` (ops lock, refresh after).
32. **Undo of rewrites = reset to the recorded HEAD + re-apply the auto-stash**, not a stash of
    the result: `UndoPoint` is taken before the op, so undo works after rebase, cherry-pick,
    revert and reset alike. Older states stay reachable through the reflog view (the undo stack
    is per session, in memory).
33. **Auto-stash is Facet's, not git's `--autostash`**: one `stash push` before the op, recorded
    in the `UndoPoint`, popped after a finished op; when an op stops half-way the stash is kept and
    the toast says so (a pop over a conflicted tree would fail or mix the two).
34. **Interactive rebase edits a list Facet builds, then hands git the finished todo** through
    Facet's own exe as `GIT_SEQUENCE_EDITOR` (same pattern as askpass, decision 27). Facet never
    parses git's own todo file; `todo_text` validates the list (a squash/fixup needs a commit
    before it) before git starts.
35. **Search, file history and reflog are filters over the History list** (`HistoryState.filter`):
    the full history is moved aside, the filtered commits/rows stream in through the same paging
    code, and leaving the filter swaps the full history back without re-running git. Ref changes
    while a filter is shown do not touch the filtered list; leaving the filter reloads the history
    only if the ref tips moved meanwhile. **Exception (memory):** a history longer than
    `history::KEEP_ASIDE_MAX` (20 000 commits) is freed instead of kept aside and read again on
    leaving the filter (~1 s on 100k, selection restored by oid) — two 100k lists at once broke
    the 300 MB budget (353 MB measured). Big lists are dropped on a helper thread
    (`free_later`), then the process heap is compacted (`platform::compact_heap`; also after
    every log read over 10 000 commits) so freed parse buffers leave private bytes.
36. **External tools are settings, not git config**: placeholder command lines per extension
    (first match wins, empty extension list = all files), run directly in a job object with temp
    files; `difftool`/`mergetool` config is never read or written (G9).
37. **Ids in long lists are `Oid`, not `String`** (`oid.rs`): inline bytes, `Copy`, hex on demand
    (`Hex` on the stack); parents are `Parents` (none / one inline / a boxed slice for merges).
    Parsers still read the hex git prints; the conversion happens once per commit in the log
    parser. Strings remain where ids are few (refs, stashes, detail, operations).
38. **LFS knowledge comes from git, not from parsing `.gitattributes`**: `git check-attr -z
    filter lockable` over the status paths answers what the LFS filter and lock rules do for a
    path, including nested attribute files and macros. Locks are read with `git lfs locks
    --json` (and `--verify` for ownership) on the credential git; nothing is cached across
    sessions.
39. **Worktree removal is never forced and is undoable by re-adding**: git refuses a dirty
    worktree (that message is shown); after a removal the toast's Undo runs `worktree add` on
    the same folder with the branch (or detached commit) it had. Tabs showing the worktree are
    closed first (their watcher holds the folder open, G32).
40. **One process per settings folder** (`instance.rs`): the first launch owns
    `\\.\pipe\facet-instance-<hash of the settings folder>`; a later launch forwards its path
    and exits before any window or runtime-heavy work. Keyed by the settings folder so isolated
    runs (`FACET_CONFIG_DIR`) never forward to the user's own Facet.
41. **Packaging is code, not a hand-kept folder**: `build.rs` writes the icon + version `.res`
    itself (no `rc.exe`, so any MSVC toolchain builds the shipping exe), and `facet-bench
    package` writes the MSIX layout, manifest and logos from the same icon code, then calls
    `makeappx`. The package is unsigned; the publisher name is a parameter so the owner's
    certificate subject can be used without code changes.
42. **Removing a repository takes it off Facet's lists; its folder only ever goes to the Recycle
    Bin** (`app_remove.rs`): the switcher's and welcome's hover × remove at once with an Undo
    toast (recent position and tab restored); "Remove Repository…" (palette, repo-button menu,
    welcome right-click, the toast's "Move to Recycle Bin…") opens a dialog whose unticked-by-
    default box also recycles the folder. Before that, `removal_check` (facet-git) lists what
    exists only there — changed/untracked files, unpushed commits per branch (one `rev-list
    --parents --branches --not --remotes`, G5), stashes, no remote — and the button turns red
    ("anyway") but is never blocked. A main worktree with linked worktrees cannot be recycled; a
    linked one can, and `worktree prune` runs on its main repo after. The move is
    `IFileOperation` with `FOFX_RECYCLEONDELETE` on an STA thread, and a progress sink aborts in
    `PreDeleteItem` unless Windows reports `TSF_DELETE_RECYCLE_IF_POSSIBLE`, so a permanent
    delete is refused (`RecycleError::WouldDelete`) rather than performed. Tabs are closed first
    (G32). Recent rows whose folder is missing or not a repository are marked (checked off the UI
    thread, again on focus regain), and a failed open of a recent offers "Remove from Recent".

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
  src/job.rs                  Windows Job Object (KILL_ON_JOB_CLOSE) around every git process (G6)
  src/askpass.rs              GIT_ASKPASS/SSH_ASKPASS bridge: named pipe server, helper_main (G7)
  src/history.rs              stream_log() (date-order pages + GraphBuilder), commit_detail(),
                              SearchKind + stream_search(), file_history() (--follow), blame()
  src/operation.rs            Operation(Kind) read from the git dir, continue/skip/abort,
                              cherry_pick, revert, reset, autostash, UndoPoint + undo_to, reflog
  src/rebase.rs               rebase_plan, TodoItem/TodoAction, todo_text, interactive_rebase,
                              rebase_onto; Editors + editor_main (GIT_SEQUENCE_EDITOR/GIT_EDITOR)
  src/conflict.rs             conflict_versions (stages 1-3), take_side, resolve_with,
                              parse_markers -> Region, apply_choices, has_markers
  src/tools.rs                external diff/merge tools: split_command, placeholders, run_tool
  src/graph.rs                GraphBuilder lane assignment, GraphRow/Segment, ascii() for tests
  src/refs.rs                 RefsSnapshot: refs + remotes + stashes, three concurrent commands
  src/branch.rs               branch/tag create, rename, delete + restore, switch, set_upstream
  src/remote.rs               add/remove/restore remote; Network: fetch, pull, push, push_tag
  src/stash.rs                stash_push (all/partial/staged), apply/pop, drop + restore
  src/clone.rs                clone (progress, cancellable, askpass/GCM), init -b, clone_folder_name
  src/forge.rs                GitHub/GitLab/Azure DevOps URLs from a remote URL
  src/oid.rs                  Oid (inline bytes, Copy), Hex, Parents — ids in the long lists
  src/lfs.rs                  lfs_version, lfs_usage, attributes (check-attr), locks / lock /
                              unlock (force), track, pull; LfsAttr, Lock
  src/submodule.rs            submodules (.gitmodules + gitlinks), update, sync, submodule_range
  src/worktree.rs             worktrees, add (existing / new branch), remove (never forced), prune
  src/removal.rs              removal_check: what exists only in a folder before it is recycled
                              (changes, unpushed per branch, stashes, remote, worktrees)
  src/parse/mod.rs            shared helpers (utf8)
  src/parse/commit.rs         COMMIT_SUMMARY_FORMAT, CommitSummary, parse_commit_summary
  src/parse/status.rs         porcelain v2 -z parser
  src/parse/diff.rs           numstat + patch parser (files -> hunks -> lines)
  src/parse/version.rs        GitVersion, parse_version
  src/parse/log.rs            LOG_FORMAT, streaming LogParser
  src/parse/refs.rs           REFS_FORMAT for-each-ref parser, remotes from config -z
  src/parse/stash.rs          STASH_FORMAT stash list parser
  src/parse/detail.rs         commit detail + diff-tree changed files
  src/parse/blame.rs          `blame --incremental` streaming parser (quoted filenames)
  src/parse/filelog.rs        `log --follow --name-status -z` file history (name per commit)
  src/parse/reflog.rs         `log -g -z` reflog entries with real parents
  src/parse/lfs.rs            check-attr -z triples, `lfs locks --json` / `lock --json` / `--verify`
  src/parse/submodule.rs      `.gitmodules` (config -z) + gitlinks (ls-files -z --stage)
  src/parse/worktree.rs       `worktree list --porcelain -z`
  tests/m4.rs                 M4 end to end: worktrees, submodules, LFS (local lock server),
                              sparse checkout + partial clone without blob fetches
  tests/common/mod.rs         TestRepo: isolated temp repo/bare repo, fixed identity + dates
  tests/repo.rs               discovery + HEAD integration tests
  tests/runner.rs             runner: streaming, stdin, cancel, logging, env, process-tree kill
  tests/history.rs            log streaming, graph on real merges, commit detail
  tests/operations.rs         M3 end-to-end: operation state, cherry-pick/revert/reset + undo,
                              conflicts, interactive rebase via the editor helper, search,
                              file history, blame, reflog, external tools
  tests/branches.rs           branches, tags, switch, remotes, fetch/pull/push, stash (bare remotes)
  tests/askpass.rs            askpass pipe protocol, token check
  tests/removal.rs            removal_check: no remote, local work, worktrees, non-roots, bare
  tests/status.rs             status: renames, unicode, spaces, conflicts, submodules, unborn
  tests/staging.rs            file/hunk/line stage + unstage, CRLF, no-EOL, new/deleted, binary
  tests/commit.rs             commit, amend, hooks (output surfaced, failure reported)
  tests/discard.rs            discard file/hunk/lines + undo, untracked files, pruning
  tests/golden.rs + golden/   parser golden fixtures (version, commit, status_v2, diff, log_graph,
                              refs, remotes, stash_list, commit_detail, diff_tree, blame,
                              file_log, reflog, check_attr, lfs_lock, lfs_locks, lfs_locks_verify,
                              gitmodules, gitlinks, worktrees)
crates/facet-bench/
  src/main.rs                 generate (linear bench repo), graph (branchy bench repo), measure,
                              history (reflog / search / file history / blame timings), package
  src/package.rs              MSIX layout, AppxManifest.xml, logos (shares icon_art.rs), makeappx
crates/facet-setup/           per-user installer / updater / uninstaller ("Install and update")
  build.rs                    embeds FACET_SETUP_PAYLOAD (or an empty payload), asInvoker
                              manifest, icon + version (shares facet's res.rs and icon_art.rs)
  src/main.rs                 install (rename-aside update), Start menu .lnk, HKCU Uninstall
                              entry, uninstall
crates/facet/
  build.rs                    embeds the app manifest (PerMonitorV2, long paths, UTF-8) and, on
                              MSVC, writes + links facet.res (icon group + VERSIONINFO)
  build/res.rs                hand-written Win32 .res writer (no rc.exe)
  tests/resources.rs          reads the exe's version resource and icons back through Win32
  src/main.rs                 rebase editor + askpass helper modes, runtime, wgpu/DX12 setup, viewport, eframe launch
  tests/askpass.rs            end-to-end: real git + facet.exe as askpass vs. a Basic-auth server
  src/app.rs                  FacetApp state, Event, RepoTab, action dispatch, persistence
  src/app_work.rs             status/diff/stage/commit/discard/undo requests and their events
  src/app_repo.rs             refs, history, detail, repo_op (branch/tag/remote/stash), network
                              jobs, askpass prompts, forge link-outs, Windows notifications;
                              repo_op / repo_op_info, Outcome
  src/app_power.rs            M3: rewrites (cherry-pick/revert/reset/rebase) + undo, operation
                              continue/skip/abort, search/file history/reflog filters, blame,
                              conflict resolver, external tools, PowerEvent handling
  src/app_trees.rs            M4: worktrees, submodules, LFS (attributes, locks, track, pull)
  src/app_add.rs              Add repository: clone (progress, cancel, cleanup) / init / open
  src/app_remove.rs           Remove repository: forget + undo, dialog state, Recycle Bin move,
                              worktree prune, recent health (missing / not a repository)
  src/app_settings.rs         M4: settings view state + actions, git path check, UI scale, code
                              font, open in, tool presets, .gitignore templates, forwarded paths
  src/integrations.rs         detect Windows Terminal / VS Code / Rider / tools; launch them
  src/gitignore.rs            Unity / Unreal templates, preview of new lines, append + undo
  src/instance.rs             single instance: named pipe per settings folder, path forwarding
  src/power.rs                per-tab PowerState: operation, undo stack, SearchBar, Blame,
                              RebaseEditor, ConflictEditor (pure, unit-tested)
  src/history.rs              per-tab HistoryState: commits + graph rows, badges, selection, detail,
                              filters (search / file history / reflog) with the full history aside
  src/bench.rs                FACET_BENCH_SCROLL frame-time benchmark, FACET_BENCH_SEARCH
  src/workspace.rs            per-tab WorkState: status, selection, open diff, line selection
  src/loader.rs               off-thread diff/image/blob loading, image decode limits
  src/watcher.rs              notify watcher, classify + debounce -> FsChange, index stamp
  src/highlight.rs            syntect highlighting (off-thread), span model
  src/textdiff.rs             word-level intra-line diff
  src/filekind.rs             extension -> Text / Image / Binary(name)
  src/actions.rs              Action enum, shortcuts, palette command list
  src/settings.rs             Settings (JSON), ThemeMode, recents, ExternalTool per extension
  src/theme.rs                tokens, Palette DARK/LIGHT, fonts, egui style install
  src/fuzzy.rs                fuzzy matcher for the palette
  src/format.rs               relative time, clock time, durations
  src/icon.rs                 window/taskbar icon (256 px from icon_art)
  src/icon_art.rs             procedural icon drawing, shared with build.rs and facet-bench
  src/platform/windows.rs     Chrome (subclass, Mica, caption, placement), folder picker, Explorer,
                              heap compaction (`compact_heap` / `release_free_memory`)
  src/platform/recycle.rs     move_to_recycle_bin: IFileOperation, refuses a permanent delete
  src/ui/mod.rs               frame layout, splitters, drop overlay
  src/ui/widgets.rs           buttons, icon buttons, elided text, skeleton, hairlines, tooltips
  src/ui/titlebar.rs          app mark, repo switcher flyout (open + recent repos, Add), branch
                              picker, "Search or run a command" box, Sync button (pull → push →
                              fetch; live progress + cancel), open-in menu, drag regions
  src/ui/sidebar.rs           nav (Changes with count, History; Settings in the footer),
                              virtualized sections: branches, remotes, tags, stashes, recent
  src/ui/content.rs           settings page switch, welcome (open / clone / new tiles, recent),
                              bare-repo overview, loading skeleton
  src/ui/changes.rs           work view layout, file list (+ context menu), perf offer, commit box
  src/ui/history.rs           commit graph list (lanes, badges, menus incl. rewrite commands),
                              filter label pills, detail pane (file menu: history/blame/tool)
  src/ui/power.rs             operation banner, search/filter bar, blame view, interactive
                              rebase editor and conflict resolver (modal layers)
  src/ui/prompt.rs            text prompt dialog (names, messages, credentials)
  src/ui/settings.rs          settings page: appearance, git, tools, open in, templates
  src/ui/add_repo.rs          Add repository dialog (Clone / Open folder / New)
  src/ui/diff.rs              diff pane: header actions, virtualized text diff, gutter, selection
  src/ui/image_diff.rs        side by side / swipe / onion skin, checkerboard
  src/ui/confirm.rs           modal confirm dialog for destructive actions
  src/ui/remove_repo.rs       Remove Repository dialog (Recycle Bin box, what would be lost)
  src/ui/command_log.rs       command log panel
  src/ui/palette.rs           command palette
  src/ui/toast.rs             toasts (one action inline beside the title; several in a row below)
```

## Where the remaining SPEC features land

Features in SPEC §2–§4 that the milestone one-liners do not name explicitly:

| Milestone | Also includes |
| --- | --- |
| M1 (done) | FS watcher (`notify`, ~150 ms debounce → targeted refresh); syntax highlighting (`syntect`) + word-level intra-line diff; binary-aware handling (.uasset/.umap/.fbx/.psd/.blend → metadata, never text diff); lazy 10 MB diffs; clipboard via `arboard`; per-repo opt-in offer for `core.fsmonitor` / `core.untrackedCache`; **benchmark repo generator** (100k files / 100k commits) and the first real perf measurements (status refresh). |
| M2 (done) | Tags; multiple remotes; fetch / pull (rebase or ff-only) / push (force-with-lease); credentials (askpass / GCM — see G7); process-tree kill via Job Objects (G6); partial stash; virtualized graph at 120 fps on the benchmark repo; Windows toast notifications for long operations; GitHub/GitLab/Azure DevOps link-outs. |
| M3 (done) | Cherry-pick, revert, reset; file history with rename following; search (message/author/file/pickaxe); per-extension external diff/merge tools (UnityYAMLMerge, P4Merge, Beyond Compare); auto-stash + reflog-based undo for destructive operations. |
| M4 (done) | Unity/Unreal `.gitignore` templates; open in Windows Terminal / VS Code / Rider; settings UI (theme, fonts, external tools, git path); sparse-checkout and partial-clone verification pass; embedded `.ico` + version resource; single-instance handling. |

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
- **G6 — process trees (resolved in M2).** Killing `git.exe` alone leaves hooks, `git-lfs`, `ssh`
  running. Every git process now runs in a Job Object with `KILL_ON_JOB_CLOSE` (`job.rs`); a
  normally finished command releases the job without killing detached children. If job creation
  fails (Facet itself in a no-nesting job) cancel falls back to killing `git.exe` only. Keep any
  new spawn path going through `GitCommand`.
- **G7 — credentials (resolved in M2).** `GIT_TERMINAL_PROMPT=0` stays set globally. Network ops
  run on `credential_git` (app.rs), which adds `GIT_ASKPASS`/`SSH_ASKPASS` = Facet's exe + pipe
  env, and `GCM_MODAL_PARENTHWND`. Do not give these variables to local commands. The decimal
  HWND format for `GCM_MODAL_PARENTHWND` is **unverified against a real GCM prompt**.
  `SSH_ASKPASS_REQUIRE=force` is set so OpenSSH uses the helper even without a display; SSH
  passphrase / host-key prompts have not been tried against a real server yet.
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
  Rust break them silently). Fat LTO takes ~5–10 min — never use `--profile dist` in the edit loop.
  Shell `sed` substitutions mangle escapes too: in M1 a sed edit turned the BOM char literal
  (backslash-u-{feff}) into `'{feff}'`. Anything containing a backslash goes through Edit/Write.
  Stop a running `facet.exe` before `cargo build --release` (the exe is locked).
  M2: clippy's `octal_escapes` fires on test literals like `"...\01700000000"` (NUL followed by
  digits); write `\x00` before a digit. Those fixes were made with Edit, never sed.
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
- **G18 — literal pathspecs per path, not globally.** `--literal-pathspecs` (or
  `GIT_LITERAL_PATHSPECS`) breaks `git stash push -u` (stash runs internal commands with `:/`
  magic). Prefix each user path with `:(literal)` instead (`stash.rs`).
- **G19 — `--date-order` is required for the graph** (children before parents) and is ~5× slower
  to first output than the default order on 100k commits (~470 vs ~95 ms). Show the first page
  quickly (200 commits) and stream the rest; do not switch to `--topo-order` (slower still) or to
  the default order (breaks incremental lanes).
- **G20 — egui menus:** close with `ui.close()` (0.37). `Response::context_menu`
  only opens on secondary click — select the row first so the action targets what was clicked.
  `ScrollArea::vertical_scroll_offset` is how to jump (used by reveal-commit and the benchmark).
- **G21 — the dev monitors run at 60 / 75 Hz**, so 120 fps cannot be observed as presented frames.
  Measure per-frame CPU (`frame.info().cpu_usage`, which in eframe 0.37 covers UI + tessellation +
  paint, excluding vsync wait) against the 8.33 ms budget instead — that is what
  `FACET_BENCH_SCROLL` reports. Re-check on a ≥ 120 Hz display when one is available.
- **G22 — windows notifications only when unfocused.** Network ops ≥ 5 s raise a Windows balloon
  (`Shell_NotifyIconW`, shown as a toast on Windows 11) only if Facet is not the foreground
  window; otherwise the in-app toast is enough. The temporary notification-area icon that
  carries it is removed (`clear_notification`) when the window is focused again.
- **G23 — never `git blame -M` (or `-C`) by default.** On the 10 MB bench file with 100k commits
  of history `blame -M` ran for more than 7 minutes; without it the whole blame takes ~4.7 s and
  the first lines arrive in ~150 ms. Facet blames with `--incremental` only (lines moved within a
  file are attributed to the moving commit). Any "detect moved lines" option must be opt-in and
  cancellable.
- **G24 — the rebase editor helper is checked before the askpass helper** in `main.rs`: git runs
  the editor with the app's environment, which (for network ops) includes the askpass variables,
  so the askpass check would claim it. Both helpers must exit before any window/runtime exists.
- **G25 — porcelain filenames can be C-quoted.** `blame --incremental` writes `filename
  "notes \303\251t\303\251.txt"` for non-ASCII / unusual names (`parse/blame.rs::unquote`).
  Prefer `-z` everywhere it exists; blame has no `-z` for that field.
- **G26 — `log --follow` takes exactly one path and no other pathspec**; renames are found by
  similarity per commit, which is why a single file's history costs ~1 s on 100k commits. Search
  by file uses an `:(icase)*text*` pathspec instead (no rename following there).
- **G27 — path / pickaxe search walk every commit's tree or diff.** On the 100k-commit bench
  repo a file search takes ~14 s and `-S` ~23 s inside git itself (`log --name-only` alone is
  ~7 s). Results stream and are cancellable (Escape, a new search, leaving the filter), and the UI
  stays responsive; do not try to speed this up with per-commit commands (G5).
- **G28 — git-lfs JSON shapes (3.5.1).** `git lfs locks --json` prints an array; `git lfs lock
  --json <path>` prints a **one-element array**, not an object (the parser accepts both);
  `locks --verify --json` prints `{"ours": [...], "theirs": [...]}`. Golden fixtures pin all
  three.
- **G29 — local submodules need `protocol.file.allow=always`.** Git ≥ 2.38 refuses `file://`
  (and plain path) submodule URLs by default. Tests pass it through `GIT_CONFIG_COUNT` /
  `GIT_CONFIG_KEY_0` on the test's own `Git`; never through global config (G9).
- **G30 — git-lfs has no machine-readable `ls-files`** (3.5.1: no `--json`). Which paths LFS
  handles comes from `git check-attr -z filter lockable` (decision 38).
- **G31 — LFS prints transfer progress only to a terminal.** Facet's git has no console, so
  network operations set `GIT_LFS_FORCE_PROGRESS=1`; LFS's lines then arrive on stderr with
  git's own progress.
- **G32 — a watched folder cannot be deleted.** The `notify` watcher of an open tab keeps a
  handle on its folder, so `git worktree remove` cannot delete the folder while the worktree
  is open in a tab. Close such tabs before removing (`app_trees.rs`).
- **G33 — the single-instance pipe is keyed by the settings folder** (decision 40). A second
  launch with a *different* `FACET_CONFIG_DIR` is a separate instance by design — the
  measurement and screenshot scripts rely on it. The forwarded path is made absolute in the
  second process (`std::path::absolute`), since the two processes have different working
  folders. The second process calls `AllowSetForegroundWindow(ASFW_ANY)` before
  forwarding; without it Windows refuses the first instance's `SetForegroundWindow` and only
  flashes the taskbar button.
- **G34 — app execution aliases are reparse points.** `wt.exe` in
  `%LOCALAPPDATA%\Microsoft\WindowsApps` fails `Path::is_file()` / `metadata()`; check it with
  `symlink_metadata` (`integrations::is_file`). Launch it by that path; it works.
- **G35 — the `.res` is linked with `cargo:rustc-link-arg-bins`** and only for
  `target_env = "msvc"` (`link.exe` takes `.res` inputs; GNU `ld` would need a COFF object).
  It coexists with `embed-manifest`'s `/MANIFEST:EMBED` because the manifest is resource type 24
  and Facet's are icon / group icon / version.
- **G36 — UI fonts lack geometric symbols.** Segoe UI Variable, Segoe UI and Consolas have no
  `◆` (U+25C6), `⋯` (U+22EF) or `⟳` (U+27F3); egui's bundled fallbacks cover only some, and a
  miss draws a box. Use an icon-font glyph (`theme::icon`) or paint the shape (the tag badge's
  diamond in `ui/history.rs`). Arrows, `−`, `…`, `·`, `—` are fine.

## Known issues / deliberate limits after M4

M4-specific:

- **The MSIX was packed and validated by `makeappx`, never installed.** It is unsigned; installing
  needs the owner's certificate (`signtool sign /fd SHA256 /f <pfx> <msix>` with a subject equal
  to `--publisher`) or Developer Mode + `Add-AppxPackage -Register layout\AppxManifest.xml`.
  Unverified inside the package: Facet's own exe as `GIT_ASKPASS` / `GIT_SEQUENCE_EDITOR` (git
  runs it from `WindowsApps`), and settings: MSIX virtualizes `%APPDATA%` writes, so the packaged
  app and the plain exe keep separate `settings.json` files.
- **True cold start (after a reboot) is still not measured**; it needs a reboot, which a session
  cannot do. The numbers below are warm-cache starts.
- **Not exercised by clicking in the app:** LFS lock / unlock / force unlock against a real LFS
  server (covered end to end against the local lock server in `tests/m4.rs`), LFS transfer
  progress with real LFS objects, submodule update / sync with network URLs, worktree add /
  remove from the UI dialogs, Browse… for the git executable (a file dialog), the external tool
  presets with the real tools, `.gitignore` append + Undo. On the manual checklist.
- **Light theme** was checked by screenshot for: welcome, changes + text diff, image diff,
  history + detail, search bar, blame, command log, palette, settings, interactive rebase
  editor, conflict resolver, operation banner. Not checked: prompt and confirm dialogs, toasts,
  swipe / onion-skin modes, context menus, the Open In menu.
- The command log shows `git diff --no-index` for untracked files in red: that command exits 1
  when the files differ, which is its success case. Cosmetic (the diff is shown correctly).
- Git LFS without a configured LFS server: locks cannot be listed; the Locks section shows git-lfs's own last
  error line (and a toast when the refresh was asked for).
- Open In detects well-known install folders and `PATH` only (no registry): Rider or VS Code
  installed somewhere custom is not offered (the settings view says what was found).
- **Remove repository:** the refusal of a permanent delete (`RecycleError::WouldDelete`) is not
  exercised. It needs a drive without a Recycle Bin or a folder over the bin's size limit. A
  real recycle is covered by an `#[ignore]` test in `platform/recycle.rs` and was checked in the
  app with an isolated `FACET_CONFIG_DIR`, along with: × in the switcher and on the welcome
  screen, broken-row labels, Undo (recent position and reopened tab), the two-button toast and
  the dialog's warnings. Not clicked: recycling a linked worktree (the prune afterwards), the
  "Remove from Recent" offer on a failed open, light theme.

Carried over from M3:

- **Not exercised by clicking in the app:** the rebase editor (incl. drag), conflict resolver,
  operation banner, blame view, external tools, cherry-pick/revert/reset/merge menus. Their git side
  is covered end to end by `tests/operations.rs` (real repos, the real editor helper), the state
  logic by unit tests in `power.rs`; on the manual checklist.
- **File and code search are slow on huge histories** (G27): ~14 s (file) and ~23 s (`-S`) on
  100k commits, all in git. They stream and cancel; first results can still take seconds.
- **Undo stack is per session** (in memory). After a restart, the reflog view is the way back.
- **Auto-stash is kept, not popped, when an op stops half-way**; the toast says so and the stash
  shows in the sidebar. Popping it is the user's step after Continue / Abort.
- **Blame does not follow lines moved between files** (no `-M`/`-C`, G23) and blames the
  working copy or one commit; no "blame parent" step yet.
- External tools: a tool that stays running blocks nothing (it runs in the background), but
  its temp files are removed only when it exits. Presets fill in only detected executables.
- Merge has no options: git's default message and its `merge.ff` setting (no `--no-ff` /
  `--squash` choice in the UI yet).

Carried over from M2:

- **Not exercised by clicking in the app:** fetch/pull/push, credential prompts, context menus,
  stash/branch/tag dialogs, Windows notifications. Their git side is covered by facet-git
  integration tests (bare remotes) and the askpass end-to-end test; the History view, sidebar
  sections and header buttons were checked by screenshot in a smoke repo. On the manual checklist.
- Real GCM and SSH prompts (G7) never ran: no network credentials were used.
- Filtering a history over 20 000 commits still frees it and re-reads it on Escape (decision
  35, ~1 s of skeleton on 100k commits). With compact oids (decision 37) keeping both lists would
  likely fit (264 MB private with a 100k search shown), but `KEEP_ASIDE_MAX` was not raised or
  re-measured.
- No UI to change a branch's upstream except publish / track (`set_upstream` exists in facet-git).
- No cap on the history: the whole log is loaded (100k commits ≈ 730 ms in the background).
- Overflow graph lanes (> what fits in 40 % of the list at 8 px) are clamped onto the last
  column; very wide histories lose lane identity there.

Carried over from M1:

- Onion-skin and swipe modes and drag-to-select lines were not checked in the light theme (logic
  is unit-tested; dark theme, side by side, swipe and click-select were checked).
- No keyboard scrolling in the diff (PgUp/PgDn/Home/End do nothing); arrow keys move between files.
- The sidebar "Recent" list is empty when all tabs were session-restored (by design, G14).
- Maximized window + auto-hide taskbar: the taskbar may not slide in (needs the
  `ABM_GETAUTOHIDEBAR` one-pixel inset in `WM_NCCALCSIZE`).
- Many open tabs are squeezed, not scrolled; the palette lists all tabs.
- Bare repositories show the overview only (no work view: there is no working tree).
- Folder picker and drag-and-drop were never exercised end-to-end by automation (M0 note); still on
  the manual checklist.
- Partial (hunk/line) staging is unavailable for renamed, conflicted, submodule and binary entries;
  conflicts can only be marked resolved (full conflict UI is M3).
- Diffs beyond 20 000 lines show without syntax/word highlighting (plain text, still virtualized).
- Commits cannot be cancelled from the UI (hooks run to completion), though the job object would
  now take the whole tree down.

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
| Log scroll 120 fps | M2, see below: per-frame CPU p99 0.99 ms (linear) / 1.14 ms (branchy), max 1.35 ms; 0 of 1 500 frames over 8.33 ms. | **Met on CPU frame time.** Presented fps capped by the 60/75 Hz monitors (G21). |

M2 `release` build, history view (`FACET_BENCH_SCROLL=1`, 1 500 frames at 37 px/frame after 120
warm-up frames; per-frame CPU = UI + tessellate + paint):

| Repo | History load (first page / all) | Frame CPU p50 / p90 / p99 / max | > 8.33 ms | Interval p50 |
| --- | --- | --- | --- | --- |
| `bench-repo` (100k commits, linear) | 476 ms / 708 ms | 0.74 / 0.85 / 0.99 / 1.27 ms | 0 | 16.66 ms (vsync) |
| `bench-graph` (100k commits, 6 029 merges, 25 lanes) | 433 ms / 730 ms | 0.90 / 1.01 / 1.14 / 1.35 ms | 0 | vsync |

- Before adaptive lanes, `bench-graph` was p50 0.90 / p99 1.18 ms (lanes > 16 piled up on one column).
- `git log` first output on 100k commits: ~95 ms default order vs ~468 ms `--date-order` (G19).
- Idle with the 100k-commit history loaded (`bench-repo`): **229 MB working set / 297 MB private**
  (budget 300 MB — tight, see known issues); idle CPU 0.00 s over 20 s.

M3 `release` build (`target/profile-m3`), `bench-repo`, one clean instance per run:

| What | Measured | Notes |
| --- | --- | --- |
| History load, then scroll | first page 576–649 ms / all 880–975 ms; frame CPU p50 0.91–0.96 / p90 1.15–1.20 / p99 1.39–1.42 / max 1.55–2.02 ms; 0 of 1 500 frames over 8.33 ms | Scroll budget **met** (CPU); M3 adds the operation banner / search bar checks per frame. |
| Idle, 100k history | **230 MB WS / 296 MB private**, idle CPU 0.09 s / 20 s | Budget 300 MB: **met, no headroom** (M2: 297). |
| Idle, 100k-result author search shown (`FACET_BENCH_SEARCH="author:bench"`) | **232 MB WS / 298 MB private**, idle CPU 0.02 s / 20 s; search first page 567 ms, all 979 ms | Before freeing the long history during a filter + heap compaction: 353 MB (over budget). |
| `facet-bench history --runs 3` | reflog 58 ms; message search 603 ms; author first page 509 / all 911 ms (100k); file search 14.1 s; `-S` 23.3 s (G27); `--follow` 1.27 s; blame of a source file first lines 79 ms / all 1.15 s; blame of the 10 MB file first lines 190 ms / all 4.98 s (140 000 lines) | Everything streams off the UI thread and cancels; only the first page gates what the user sees. |

M4 `release` build (`target/profile-m4`, thin LTO) unless noted, one clean instance per run,
`FACET_BENCH_SCROLL` = 120 warm-up + 1 500 measured frames, idle sampled over 20 s:

| Budget / case | Measured | Verdict |
| --- | --- | --- |
| Cold start < 1 s | First frame 584–649 ms (bench-repo restored, release); **`dist` exe 579 / 655 / 655 ms** (3 runs), 596 ms for the final rebuilt `dist` exe. All warm cache, GT 1030 / DX12. | Met warm. **True cold start (after reboot) not measured.** |
| Warm status refresh < 150 ms | In-app first refresh 220–261 ms (the first one after launch, cold git caches); warm numbers not re-measured since M1 (111 ms median), no status code changed in M4. | First refresh over 150 ms, as in M1 (254 ms there); warm met in M1. |
| 10 MB diff scroll (`diff:Docs/Generated/Big.txt`, 159 998 rows) | frame CPU p50 1.29 / p90 1.60 / p99 1.89 / max 2.34 ms; **0 of 1 500 over 8.33 ms**; interval p50 13.38 ms; load 323 ms (11.6 MB), decorate 6 ms | **Met** (CPU frame time, G21). First measurement of this budget. |
| History scroll, `bench-repo` (100k, linear) | 2 runs: first page 559–562 / all 828–854 ms; p50 0.93 / p90 1.14–1.19 / p99 1.38–1.48 / max 2.38–2.69 ms; 0 over | **Met** |
| History scroll, `bench-graph` (25 lanes) | first page 865 / all 1 208 ms; p50 1.11 / p90 1.43 / p99 1.75 / max 3.51 ms; 0 over | **Met** |
| Idle RAM < 300 MB, 100k history | **207–211 MB WS / 263–266 MB private** (M3: 296) | Met, ~35 MB headroom (compact oids) |
| Idle RAM, 100k-result author search shown | **207 MB WS / 264 MB private** (M3: 298); search first page 516 / all 898 ms; scroll p99 1.50 ms, 0 over | Met |
| Idle RAM, `bench-graph` history | **227 MB WS / 285 MB private** | Met — the tightest case |
| Idle RAM, 10 MB diff open | 190 MB WS / 255 MB private (M1: 253) | Met |
| Idle RAM, `dist` exe, file list only | 166–210 MB WS / 224–267 MB private (3 runs, 5 s idle; varies with how far the history load has got) | Met |
| Idle CPU | 0.02–0.05 s / 20 s in most runs; outliers 0.94 s (bench-repo) and 0.58 s (bench-graph) in single runs, not reproduced in the repeat run | Met in steady state; the outliers are unexplained (likely the background log read / heap compaction finishing inside the window) |

- Single exe: `target/dist/facet.exe` 14.7 MB (fat LTO), no files beside it. MSIX: 6.8 MB.
- Bench numbers are noisier than M3's: first frames 584–655 ms vs 576–649 ms in M3 — no
  regression from the single-instance pipe check or the bigger icon work.

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

## Next steps (M4.1 — release hardening)

M4 closed the SPEC feature list. M4.1 is defined here, not in SPEC: make the release path real
and close what M4 could not verify inside a session. No new features beyond SPEC §4.

Suggested order:

1. **MSIX install:** sign with a test certificate whose subject equals `--publisher` (or
   Developer Mode + `Add-AppxPackage -Register`), install, launch from Start and from the
   `facet.exe` alias. Verify inside the package: Facet's exe as `GIT_ASKPASS` and
   `GIT_SEQUENCE_EDITOR` (git runs it from `WindowsApps` — fetch from an authenticated remote,
   an interactive rebase), the single-instance pipe, and where `settings.json` lands (MSIX
   `%APPDATA%` virtualization). Fix what fails; document the signing steps the owner runs.
2. **True cold start** after a reboot (dist exe, bench repo restored); if over 1 s, profile it.
3. **Click through the unclicked flows** from the M2–M4 known issues (LFS lock/unlock in the UI,
   worktree add/remove + undo dialogs, submodule update, `.gitignore` append + Undo, tool
   presets, Browse… for git, folder picker, drag and drop, fetch/pull/push, rebase editor drag,
   conflict resolver); fix the bugs found.
4. **Light theme** of the remaining surfaces: prompt and confirm dialogs, toasts, swipe /
   onion skin, context menus, the Open In menu.
5. **Small known issues:** `git diff --no-index` exit 1 shown red in the command log (it is
   success there); re-measure keeping 100k histories aside during a filter now that oids are
   compact (`KEEP_ASIDE_MAX`); the idle-CPU outliers (find what runs in that window).
6. Re-measure every budget with the `dist` exe and update this file.
