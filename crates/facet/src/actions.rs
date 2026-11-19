//! Every user-invokable action in Facet.
//!
//! Buttons, keyboard shortcuts and the command palette all go through [`Action`], so the
//! palette reaches everything by construction: a feature is added by adding a variant here,
//! listing it in [`commands`], and handling it in `FacetApp::run`.

use std::path::PathBuf;

use eframe::egui::{Key, KeyboardShortcut, Modifiers};
use facet_git::conflict::{Choice, Side};
use facet_git::forge::ForgeKind;
use facet_git::parse::{LfsLock, Remote, StashEntry};
use facet_git::rebase::TodoAction;
use facet_git::{
    DeletedBranch, DeletedTag, DiscardFile, OperationKind, PerfConfig, PushRequest, ResetMode,
    SearchKind, StashOptions, SwitchTarget, WorktreeTarget,
};

use crate::app_add::AddTab;
use crate::gitignore::{Template, Written};
use crate::history::View;
use crate::integrations::{OpenTarget, ToolPreset};
use crate::power::UndoItem;
use crate::settings::{
    CodeFont, Density, Editor, ExternalTool, ImageMode, PullMode, Terminal, ThemeMode,
    fetch_interval_label, CODE_FONT_SIZES, FETCH_INTERVALS, UI_SCALES,
};
use crate::workspace::Area;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    ShowPalette,
    /// The native folder picker, straight to opening a repository.
    OpenRepository,
    /// The Add Repository dialog on a tab (`None`: the tab it was last closed on).
    ShowAddRepository(Option<AddTab>),
    CloseAddRepository,
    /// Browse… in the dialog: where to clone / create, or the folder to open.
    BrowseAddFolder,
    StartClone,
    CancelClone,
    CreateRepository,
    /// Open the folder typed into the dialog's Open folder tab.
    OpenTypedPath,
    /// The title bar's Sync button: pull when behind, else push when ahead or unpublished,
    /// else fetch.
    Sync,
    OpenPath(PathBuf),
    ForgetRecent(PathBuf),
    CloseTab(usize),
    CloseActiveTab,
    NextTab,
    PreviousTab,
    SwitchTab(usize),
    Refresh,
    RevealInExplorer,
    CopyRepositoryPath,
    ToggleSidebar,
    ToggleCommandLog,
    ClearCommandLog,
    CopyText {
        text: String,
        what: &'static str,
    },
    SetTheme(ThemeMode),
    Quit,

    // ----- working copy: file list ---------------------------------------------------------
    /// A click on a file row (`toggle`: Ctrl, `extend`: Shift).
    SelectFile {
        area: Area,
        index: usize,
        toggle: bool,
        extend: bool,
    },
    SelectNextFile,
    SelectPreviousFile,
    /// Shift+Arrow: grow the selection by one row up (-1) or down (1).
    ExtendFileSelection(isize),
    SelectAllFiles,
    StageSelected,
    UnstageSelected,
    /// Stage the selection if it is in "Changes", unstage it if it is in "Staged".
    ToggleStageSelected,
    StagePaths(Vec<String>),
    UnstagePaths(Vec<String>),
    StageAll,
    UnstageAll,
    /// Discards ask for confirmation first; the `Confirmed…` variants do the work.
    DiscardSelected,
    DiscardAll,
    DiscardPath(String),
    ConfirmedDiscardFiles {
        files: Vec<DiscardFile>,
        label: String,
    },
    ConfirmedDiscardPatch {
        patch: Vec<u8>,
        label: String,
    },
    /// Undo the latest discard (or redo it, right after an undo).
    UndoDiscard,

    // ----- working copy: diff --------------------------------------------------------------
    StageHunk(usize),
    UnstageHunk(usize),
    DiscardHunk(usize),
    StageLines,
    UnstageLines,
    DiscardLines,
    ClearLineSelection,
    CopySelectedLines,
    CopyFilePath,
    SetImageMode(ImageMode),
    /// Text diffs side by side (`true`) or unified.
    SetSplitDiff(bool),

    // ----- working copy: commit ------------------------------------------------------------
    Commit,
    ToggleAmend,
    FocusCommitMessage,
    DismissCommitError,

    // ----- per-repository performance settings ---------------------------------------------
    EnablePerf,
    DisablePerf,
    DeclinePerfOffer,

    // ----- history ---------------------------------------------------------------------------
    ShowChanges,
    ShowHistory,
    SelectCommit(usize),
    SelectNextCommit,
    SelectPreviousCommit,
    /// Show this commit in the history (a branch, tag or stash clicked in the sidebar).
    RevealCommit(String),
    SelectCommitFile(usize),
    /// Back from a file's diff to the commit it belongs to.
    CloseCommitFile,
    /// Show the files of this stash (its position in the list) and their diffs.
    ShowStash(usize),
    SelectStashFile(usize),
    ToggleSidebarSection(String),

    // ----- branches and tags -----------------------------------------------------------------
    /// Ask for a name; the branch starts at `at` (a commit) or at HEAD.
    NewBranch {
        at: Option<String>,
    },
    CreateBranch {
        name: String,
        at: String,
        switch: bool,
    },
    SwitchBranch(String),
    /// Open the branch flyout under the branch button of the title bar.
    ShowBranches,
    /// Check out a remote-tracking branch (`origin/feature`) as a new local branch.
    CheckoutRemoteBranch(String),
    CheckoutCommit(String),
    /// A switch was refused because of local changes: stash them, then switch.
    StashAndSwitch(SwitchTarget),
    RenameBranch(String),
    ConfirmedRenameBranch {
        old: String,
        new: String,
    },
    DeleteBranch(String),
    ConfirmedDeleteBranch(String),
    RestoreBranch(DeletedBranch),
    NewTag {
        at: Option<String>,
    },
    CreateTag {
        name: String,
        at: String,
        message: Option<String>,
    },
    DeleteTag(String),
    ConfirmedDeleteTag(String),
    RestoreTag(DeletedTag),
    PushTag(String),

    // ----- remotes and the network -----------------------------------------------------------
    AddRemote,
    CreateRemote {
        name: String,
        url: String,
    },
    RemoveRemote(String),
    ConfirmedRemoveRemote(String),
    RestoreRemote(Remote),
    Fetch,
    FetchRemote(String),
    Pull,
    Push,
    /// Ask before overwriting the remote branch (`--force-with-lease` only).
    ForcePush,
    ConfirmedForcePush(PushRequest),
    CancelNetwork,
    SetPullMode(PullMode),

    // ----- stash -----------------------------------------------------------------------------
    /// Ask for a message, then stash every change.
    Stash,
    /// Ask for a message, then stash only the selected files.
    StashSelected,
    StashStaged,
    CreateStash(StashOptions),
    ApplyStash(usize),
    PopStash(usize),
    DropStash(usize),
    ConfirmedDropStash(StashEntry),
    RestoreStash(StashEntry),

    // ----- link-outs -------------------------------------------------------------------------
    OpenCommitOnForge(String),
    OpenBranchOnForge(String),
    CreatePullRequest(String),

    // ----- rewriting history (M3) ------------------------------------------------------------
    /// Copy a commit onto the current branch (local changes are auto-stashed around it).
    CherryPick(String),
    /// Make a commit that undoes this one.
    Revert(String),
    /// Merge a branch (or commit) into the current branch (local changes are auto-stashed).
    MergeIntoCurrent(String),
    /// Move the current branch to a commit. A hard reset asks first.
    ResetTo {
        oid: String,
        mode: ResetMode,
    },
    ConfirmedResetTo {
        oid: String,
        mode: ResetMode,
    },
    /// Rebase the current branch onto a commit (asks first).
    RebaseOnto(String),
    ConfirmedRebaseOnto(String),
    /// Open the interactive rebase editor for the commits after `base` (`None`: all of them).
    InteractiveRebase {
        base: Option<String>,
    },
    SetRebaseStep {
        row: usize,
        action: TodoAction,
    },
    /// Move a row of the rebase editor up (-1) or down (1).
    RebaseMoveRow {
        row: usize,
        step: isize,
    },
    StartRebase,
    CancelRebase,
    /// Continue / skip a step of / abandon the rebase, merge, cherry-pick… in progress.
    ContinueOperation,
    SkipOperation,
    AbortOperation,
    ConfirmedAbortOperation,
    /// Put HEAD back where it was before an operation (re-applying an auto-stash).
    UndoOperation(UndoItem),
    UndoLastOperation,

    // ----- search, file history, reflog, blame -------------------------------------------------
    /// Open the search field above the commit list.
    FindCommits,
    SetSearchKind(SearchKind),
    RunSearch,
    /// Back to the full history from search results, a file's history or the reflog.
    ClearFilter,
    /// The history shows every branch (`true`) or only the checked-out one.
    SetHistoryAllBranches(bool),
    ShowFileHistory(String),
    ShowReflog,
    /// Who changed each line of a file: in the working copy (`rev: None`) or at a commit.
    Blame {
        path: String,
        rev: Option<String>,
    },
    /// Blame / history of the file shown in the diff (working copy or selected commit).
    BlameCurrentFile,
    FileHistoryCurrentFile,
    CloseBlame,

    // ----- conflicts and external tools --------------------------------------------------------
    /// Resolve a conflicted file by taking one side whole.
    TakeSide {
        path: String,
        side: Side,
    },
    /// Open the chunk-by-chunk resolver for a conflicted file.
    ResolveConflicts(String),
    ChooseChunk {
        chunk: usize,
        choice: Option<Choice>,
    },
    ChooseAllChunks(Choice),
    /// Write the resolver's result and mark the file resolved.
    SaveResolution,
    CloseConflictEditor,
    OpenMergeTool(String),
    /// Open the shown diff in the external diff tool for its file type.
    OpenDiffTool,
    AddExternalTool,
    CreateExternalTool(ExternalTool),
    RemoveExternalTool(usize),

    // ----- worktrees, submodules, Git LFS (M4) ---------------------------------------------
    /// Ask for a folder and a branch for a new worktree (`branch` prefills the dialog).
    AddWorktree {
        branch: Option<String>,
    },
    CreateWorktree {
        path: PathBuf,
        branch: String,
    },
    RemoveWorktree(PathBuf),
    ConfirmedRemoveWorktree {
        path: PathBuf,
        restore: WorktreeTarget,
    },
    /// Undo of a removal: add the worktree again.
    RestoreWorktree {
        path: PathBuf,
        target: WorktreeTarget,
    },
    PruneWorktrees,
    /// `submodule update --init --recursive` for one submodule (or all).
    UpdateSubmodules(Option<String>),
    SyncSubmodules(Option<String>),
    /// Open a checked-out submodule as a tab (repository-relative path).
    OpenSubmodule(String),
    RefreshLocks,
    LockFile(String),
    UnlockFile {
        id: String,
        path: String,
    },
    /// Break someone else's lock (asks first).
    ForceUnlock {
        id: String,
        path: String,
        owner: String,
    },
    ConfirmedForceUnlock {
        id: String,
        path: String,
    },
    LfsPull,
    /// Ask for a pattern to store with LFS.
    TrackWithLfs,
    CreateLfsTrack {
        pattern: String,
        lockable: bool,
    },

    // ----- settings and integrations (M4) -------------------------------------------------
    ShowSettings,
    /// Settings' Advanced page: git executable, interface size, tools, templates.
    ShowAdvancedSettings,
    CloseSettings,
    /// Use the accent color picked in Windows (or Facet's own blue).
    SetWindowsAccent(bool),
    SetCodeFontSize(f32),
    SetDensity(Density),
    /// Fetch every open repository in the background.
    SetAutoFetch(bool),
    /// How often the background fetch runs, in minutes.
    SetFetchMinutes(u32),
    /// The first branch of repositories created in Facet.
    SetDefaultBranch(String),
    SetEditor(Editor),
    SetTerminal(Terminal),
    /// Sign in to a hosting service through Git Credential Manager (`github.com`).
    SignIn(String),
    /// Make the credential helper forget the account on a host.
    SignOut(String),
    /// Ask for the commit author's name and email (kept in the repository's own config).
    EditCommitAuthor,
    SetCommitAuthor {
        name: String,
        email: String,
    },
    /// How the active repository converts line endings (`core.autocrlf`, repo-local).
    SetLineEndings(facet_git::LineEndings),
    /// Copy versions and paths for a bug report.
    CopyDiagnostics,
    /// The Windows Credential Manager control panel.
    ManageCredentials,
    /// Open the repository in the terminal chosen in settings.
    OpenTerminal,
    /// Open the repository (or a file in it) in the editor chosen in settings.
    OpenInEditor(Option<String>),
    /// Interface size (egui zoom factor).
    SetUiScale(f32),
    SetCodeFont(CodeFont),
    /// Check the executable with `git version`, then use it (`None`: git on `PATH`).
    SetGitPath(Option<PathBuf>),
    /// Pick git.exe with the file dialog, then [`Action::SetGitPath`].
    BrowseGitPath,
    /// The add-tool dialog filled in for a known diff / merge tool.
    AddExternalToolPreset(ToolPreset),
    /// Open the repository in another program.
    OpenIn(OpenTarget),
    /// Settings, scrolled to the `.gitignore` preview for this template.
    ShowGitignoreTemplate(Template),
    AddGitignoreTemplate(Template),
    /// Undo of [`Action::AddGitignoreTemplate`].
    RestoreGitignore {
        root: PathBuf,
        change: Written,
    },

    // ----- the text prompt dialog ------------------------------------------------------------
    SubmitPrompt,
    CancelPrompt,
}

pub mod shortcut {
    use super::{Key, KeyboardShortcut, Modifiers};

    const CTRL_SHIFT: Modifiers = Modifiers::CTRL.plus(Modifiers::SHIFT);

    pub const PALETTE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::K);
    /// The palette's second shortcut, as in code editors.
    pub const PALETTE_ALT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::P);
    pub const SETTINGS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Comma);
    pub const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::O);
    pub const CLOSE_TAB: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::W);
    pub const NEXT_TAB: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Tab);
    pub const PREVIOUS_TAB: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::Tab);
    pub const REFRESH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F5);
    pub const REVEAL: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::E);
    pub const COPY_PATH: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::C);
    pub const SIDEBAR: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::B);
    pub const COMMAND_LOG: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Backtick);

    pub const SHOW_CHANGES: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Num1);
    pub const SHOW_HISTORY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Num2);
    pub const FETCH: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::F);
    pub const PULL: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::L);
    pub const PUSH: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::P);
    pub const NEW_BRANCH: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::N);
    pub const STASH: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::S);
    pub const SEARCH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::F);
    pub const UNDO_OPERATION: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::Z);
    pub const BLAME: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::B);
    pub const FILE_HISTORY: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::H);
    pub const REFLOG: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::R);

    pub const COMMIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Enter);
    pub const FOCUS_MESSAGE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::M);
    pub const STAGE_ALL: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::A);
    pub const UNSTAGE_ALL: KeyboardShortcut = KeyboardShortcut::new(CTRL_SHIFT, Key::U);

    // The shortcuts below belong to the file list and the diff: they apply while no text
    // field or button has keyboard focus (see `FacetApp::handle_work_keys`).
    pub const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Z);
    pub const SELECT_ALL: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::A);
    pub const TOGGLE_STAGE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Enter);
    pub const DISCARD: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Delete);
    pub const NEXT_FILE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowDown);
    pub const PREVIOUS_FILE: KeyboardShortcut =
        KeyboardShortcut::new(Modifiers::NONE, Key::ArrowUp);
    pub const CLEAR_LINES: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Escape);
    pub const COPY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::C);
}

/// One entry in the command palette.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub title: String,
    pub shortcut: Option<KeyboardShortcut>,
    pub action: Action,
}

/// The state of the active repository's working copy, as far as commands depend on it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkContext {
    pub staged: usize,
    pub unstaged: usize,
    /// Where the file selection is, and how many files it holds.
    pub selection: Option<(Area, usize)>,
    /// A file is shown in the diff pane.
    pub has_current: bool,
    /// Which list the shown diff belongs to.
    pub diff_area: Option<Area>,
    /// Hunks of the shown diff that can be staged / unstaged / discarded on their own.
    pub hunks: usize,
    pub selected_lines: usize,
    pub can_commit: bool,
    pub can_amend: bool,
    pub amend: bool,
    /// The latest discard: its label, and whether invoking it redoes rather than undoes.
    pub undo: Option<(String, bool)>,
    pub perf: Option<PerfConfig>,
    /// An image comparison is on screen (and in which mode).
    pub image_mode: Option<ImageMode>,
}

/// Branches, remotes, stashes and history of the active repository, as far as commands
/// depend on them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepoContext {
    pub view: View,
    /// Bare repositories have no working tree: no switching or stashing.
    pub bare: bool,
    /// The checked-out branch (`None`: detached or unborn without a name).
    pub branch: Option<String>,
    /// HEAD points at a commit (the repository is not empty).
    pub has_commits: bool,
    pub upstream: Option<String>,
    /// Local branches other than the checked-out one.
    pub local_branches: Vec<String>,
    /// Remote-tracking branches (`origin/main`).
    pub remote_branches: Vec<String>,
    pub tags: Vec<String>,
    pub remotes: Vec<String>,
    /// Stash messages, newest first (the position is the stash index).
    pub stashes: Vec<String>,
    /// The commit selected in the history: its id and short id.
    pub selected_commit: Option<(String, String)>,
    pub network_running: bool,
    pub pull_mode: PullMode,
    /// The history shows every branch, not only the checked-out one.
    pub all_branches: bool,
    /// A file of the selected commit is open.
    pub commit_file_open: bool,
    pub forge: Option<ForgeKind>,
    /// The working tree has changes to stash.
    pub dirty: bool,
    /// Changes in the index (for "Stash Staged Changes").
    pub staged: bool,
    /// The rebase, merge, cherry-pick… in progress.
    pub operation: Option<OperationKind>,
    /// The newest operation that can be undone.
    pub undo: Option<String>,
    /// The commit list shows search results, a file's history or the reflog (its title).
    pub filter: Option<String>,
    /// The file shown in the diff: its path and the commit it is shown at (`None`: the
    /// working copy).
    pub current_file: Option<(String, Option<String>)>,
    /// The shown file is conflicted (its path).
    pub conflicted_file: Option<String>,
    /// An external merge tool is set up for the conflicted file.
    pub merge_tool: bool,
    /// An external diff tool is set up for the shown file.
    pub diff_tool: bool,
    /// External tools, as labelled in settings.
    pub external_tools: Vec<String>,
    /// The repository's other worktrees: name and folder.
    pub worktrees: Vec<(String, PathBuf)>,
    /// Some worktree's folder is gone (prune would forget it).
    pub prunable_worktrees: bool,
    /// Submodules: path and whether checked out.
    pub submodules: Vec<(String, bool)>,
    /// The repository uses Git LFS.
    pub lfs: bool,
    pub lfs_locks: Vec<LfsLock>,
}

/// What the command list depends on.
pub struct CommandContext<'a> {
    /// Display names of the open tabs, in order.
    pub tabs: &'a [String],
    pub active_tab: Option<usize>,
    /// The active tab has finished loading.
    pub repo_ready: bool,
    /// The active repository's working copy (none for bare repositories).
    pub work: Option<&'a WorkContext>,
    /// Branches, remotes, stashes and history of the active repository.
    pub repo: Option<&'a RepoContext>,
    pub recent: &'a [PathBuf],
    pub theme: ThemeMode,
    pub sidebar_visible: bool,
    pub command_log_visible: bool,
    pub command_log_empty: bool,
    pub ui_scale: f32,
    pub code_font: CodeFont,
    pub code_font_size: f32,
    pub windows_accent: bool,
    pub density: Density,
    pub auto_fetch: bool,
    /// A git executable is chosen in settings (not the one on `PATH`).
    pub custom_git: bool,
    /// Programs the repository can be opened in (installed ones).
    pub open_in: &'a [OpenTarget],
}

/// Palette entries are listed for at most this many hunks of a diff.
const MAX_HUNK_COMMANDS: usize = 30;

/// All commands available right now, in palette order. Commands that cannot run in the current
/// state are left out rather than shown disabled.
pub fn commands(cx: &CommandContext<'_>) -> Vec<Command> {
    let mut list = Vec::new();
    let mut add = |title: String, shortcut: Option<KeyboardShortcut>, action: Action| {
        list.push(Command {
            title,
            shortcut,
            action,
        });
    };

    add(
        "Add Repository…".into(),
        Some(shortcut::OPEN),
        Action::ShowAddRepository(None),
    );
    add(
        "Clone Repository…".into(),
        None,
        Action::ShowAddRepository(Some(AddTab::Clone)),
    );
    add(
        "New Repository…".into(),
        None,
        Action::ShowAddRepository(Some(AddTab::New)),
    );
    add(
        "Open Repository Folder…".into(),
        None,
        Action::OpenRepository,
    );

    if let Some(work) = cx.work {
        work_commands(work, &mut add);
    }
    if let Some(repo) = cx.repo {
        repo_commands(repo, &mut add);
    }

    if cx.repo_ready {
        add("Refresh".into(), Some(shortcut::REFRESH), Action::Refresh);
        add(
            "Open in File Explorer".into(),
            Some(shortcut::REVEAL),
            Action::RevealInExplorer,
        );
        add(
            "Copy Repository Path".into(),
            Some(shortcut::COPY_PATH),
            Action::CopyRepositoryPath,
        );
        add("Open in Terminal".into(), None, Action::OpenTerminal);
        add("Open in External Editor".into(), None, Action::OpenInEditor(None));
        for target in cx.open_in {
            add(
                format!("Open in {}", target.label()),
                None,
                Action::OpenIn(*target),
            );
        }
    }
    if cx.work.is_some() {
        for template in Template::ALL {
            add(
                format!("Add {} Rules to .gitignore…", template.label()),
                None,
                Action::ShowGitignoreTemplate(template),
            );
        }
    }

    if cx.active_tab.is_some() {
        add(
            "Close Tab".into(),
            Some(shortcut::CLOSE_TAB),
            Action::CloseActiveTab,
        );
    }
    if cx.tabs.len() > 1 {
        add("Next Tab".into(), Some(shortcut::NEXT_TAB), Action::NextTab);
        add(
            "Previous Tab".into(),
            Some(shortcut::PREVIOUS_TAB),
            Action::PreviousTab,
        );
        for (index, name) in cx.tabs.iter().enumerate() {
            if Some(index) != cx.active_tab {
                add(
                    format!("Switch to Tab: {name}"),
                    None,
                    Action::SwitchTab(index),
                );
            }
        }
    }

    add(
        if cx.sidebar_visible {
            "Hide Sidebar"
        } else {
            "Show Sidebar"
        }
        .into(),
        Some(shortcut::SIDEBAR),
        Action::ToggleSidebar,
    );
    add(
        if cx.command_log_visible {
            "Hide Command Log"
        } else {
            "Show Command Log"
        }
        .into(),
        Some(shortcut::COMMAND_LOG),
        Action::ToggleCommandLog,
    );
    if !cx.command_log_empty {
        add("Clear Command Log".into(), None, Action::ClearCommandLog);
    }

    add(
        "Settings…".into(),
        Some(shortcut::SETTINGS),
        Action::ShowSettings,
    );
    for mode in ThemeMode::ALL {
        if mode != cx.theme {
            add(
                format!("Theme: {}", mode.label()),
                None,
                Action::SetTheme(mode),
            );
        }
    }
    for scale in UI_SCALES {
        if (scale - cx.ui_scale).abs() > 0.001 {
            add(
                format!("Interface Size: {:.0}%", scale * 100.0),
                None,
                Action::SetUiScale(scale),
            );
        }
    }
    for font in CodeFont::ALL {
        if font != cx.code_font {
            add(
                format!("Code Font: {}", font.label()),
                None,
                Action::SetCodeFont(font),
            );
        }
    }
    for size in CODE_FONT_SIZES {
        if (size - cx.code_font_size).abs() > 0.01 {
            add(
                format!("Code Font Size: {size:.0}"),
                None,
                Action::SetCodeFontSize(size),
            );
        }
    }
    add(
        if cx.windows_accent {
            "Accent Color: Facet Blue"
        } else {
            "Accent Color: Use the Windows Accent"
        }
        .into(),
        None,
        Action::SetWindowsAccent(!cx.windows_accent),
    );
    for density in Density::ALL {
        if density != cx.density {
            add(
                format!("Density: {}", density.label()),
                None,
                Action::SetDensity(density),
            );
        }
    }
    add(
        if cx.auto_fetch {
            "Stop Fetching in the Background"
        } else {
            "Fetch in the Background"
        }
        .into(),
        None,
        Action::SetAutoFetch(!cx.auto_fetch),
    );
    for minutes in FETCH_INTERVALS {
        add(
            format!("Background Fetch: {}", fetch_interval_label(minutes)),
            None,
            Action::SetFetchMinutes(minutes),
        );
    }
    for editor in Editor::ALL {
        add(
            format!("External Editor: {}", editor.label()),
            None,
            Action::SetEditor(editor),
        );
    }
    for terminal in Terminal::ALL {
        add(
            format!("Terminal: {}", terminal.label()),
            None,
            Action::SetTerminal(terminal),
        );
    }
    for (name, host) in [("GitHub", "github.com"), ("GitLab", "gitlab.com")] {
        add(format!("Sign In to {name}"), None, Action::SignIn(host.into()));
        add(format!("Sign Out of {name}"), None, Action::SignOut(host.into()));
    }
    add("Manage Saved Credentials".into(), None, Action::ManageCredentials);
    add("Commit Author…".into(), None, Action::EditCommitAuthor);
    add("Copy Diagnostics".into(), None, Action::CopyDiagnostics);
    add(
        "Settings: Advanced".into(),
        None,
        Action::ShowAdvancedSettings,
    );
    add("Choose Git Executable…".into(), None, Action::BrowseGitPath);
    if cx.custom_git {
        add("Use Git from PATH".into(), None, Action::SetGitPath(None));
    }
    for preset in ToolPreset::ALL {
        add(
            format!("Add External Tool: {}…", preset.label()),
            None,
            Action::AddExternalToolPreset(preset),
        );
    }

    for path in cx.recent {
        // Name first so it survives truncation; the full path disambiguates same-named repos.
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        add(
            format!("Open Recent: {name}  —  {}", path.display()),
            None,
            Action::OpenPath(path.clone()),
        );
    }

    add("Quit Facet".into(), None, Action::Quit);
    list
}

fn work_commands(
    work: &WorkContext,
    add: &mut impl FnMut(String, Option<KeyboardShortcut>, Action),
) {
    if work.can_commit {
        add(
            if work.amend {
                "Amend Last Commit"
            } else {
                "Commit"
            }
            .into(),
            Some(shortcut::COMMIT),
            Action::Commit,
        );
    }
    add(
        "Write Commit Message".into(),
        Some(shortcut::FOCUS_MESSAGE),
        Action::FocusCommitMessage,
    );
    if work.can_amend {
        add(
            if work.amend {
                "Commit: Stop Amending"
            } else {
                "Commit: Amend Last Commit"
            }
            .into(),
            None,
            Action::ToggleAmend,
        );
    }

    if work.unstaged > 0 {
        add(
            "Stage All Changes".into(),
            Some(shortcut::STAGE_ALL),
            Action::StageAll,
        );
    }
    if work.staged > 0 {
        add(
            "Unstage All Changes".into(),
            Some(shortcut::UNSTAGE_ALL),
            Action::UnstageAll,
        );
    }
    match work.selection {
        Some((Area::Unstaged, count)) if count > 0 => {
            add(
                format!("Stage {}", selected_files(count)),
                Some(shortcut::TOGGLE_STAGE),
                Action::StageSelected,
            );
            add(
                format!("Discard {}…", selected_files(count)),
                Some(shortcut::DISCARD),
                Action::DiscardSelected,
            );
        }
        Some((Area::Staged, count)) if count > 0 => add(
            format!("Unstage {}", selected_files(count)),
            Some(shortcut::TOGGLE_STAGE),
            Action::UnstageSelected,
        ),
        _ => {}
    }
    if work.unstaged > 0 {
        add("Discard All Changes…".into(), None, Action::DiscardAll);
    }
    if let Some((label, redo)) = &work.undo {
        add(
            if *redo {
                format!("Redo Discard: {label}")
            } else {
                format!("Undo Discard: {label}")
            },
            Some(shortcut::UNDO),
            Action::UndoDiscard,
        );
    }

    // Lines and hunks of the diff on screen.
    if work.selected_lines > 0 {
        let lines = if work.selected_lines == 1 {
            "Selected Line".to_owned()
        } else {
            format!("{} Selected Lines", work.selected_lines)
        };
        match work.diff_area {
            Some(Area::Unstaged) => {
                add(
                    format!("Stage {lines}"),
                    Some(shortcut::TOGGLE_STAGE),
                    Action::StageLines,
                );
                add(
                    format!("Discard {lines}…"),
                    Some(shortcut::DISCARD),
                    Action::DiscardLines,
                );
            }
            Some(Area::Staged) => add(
                format!("Unstage {lines}"),
                Some(shortcut::TOGGLE_STAGE),
                Action::UnstageLines,
            ),
            None => {}
        }
        add(
            format!("Copy {lines}"),
            Some(shortcut::COPY),
            Action::CopySelectedLines,
        );
        add(
            "Clear Line Selection".into(),
            Some(shortcut::CLEAR_LINES),
            Action::ClearLineSelection,
        );
    }
    for hunk in 0..work.hunks.min(MAX_HUNK_COMMANDS) {
        let number = hunk + 1;
        match work.diff_area {
            Some(Area::Unstaged) => {
                add(
                    format!("Stage Hunk {number}"),
                    None,
                    Action::StageHunk(hunk),
                );
                add(
                    format!("Discard Hunk {number}…"),
                    None,
                    Action::DiscardHunk(hunk),
                );
            }
            Some(Area::Staged) => add(
                format!("Unstage Hunk {number}"),
                None,
                Action::UnstageHunk(hunk),
            ),
            None => {}
        }
    }

    if work.staged + work.unstaged > 0 {
        add(
            "Select Next File".into(),
            Some(shortcut::NEXT_FILE),
            Action::SelectNextFile,
        );
        add(
            "Select Previous File".into(),
            Some(shortcut::PREVIOUS_FILE),
            Action::SelectPreviousFile,
        );
        add(
            "Select All Files in List".into(),
            Some(shortcut::SELECT_ALL),
            Action::SelectAllFiles,
        );
    }
    if work.has_current {
        add("Copy File Path".into(), None, Action::CopyFilePath);
        add("Diff Layout: Unified".into(), None, Action::SetSplitDiff(false));
        add("Diff Layout: Split".into(), None, Action::SetSplitDiff(true));
    }
    if let Some(current) = work.image_mode {
        for mode in ImageMode::ALL {
            if mode != current {
                add(
                    format!("Image Diff: {}", mode.label()),
                    None,
                    Action::SetImageMode(mode),
                );
            }
        }
    }

    match work.perf {
        Some(config) if config.all_enabled() => add(
            "Repository: Disable File System Monitor and Untracked Cache".into(),
            None,
            Action::DisablePerf,
        ),
        Some(_) => add(
            "Repository: Enable File System Monitor and Untracked Cache".into(),
            None,
            Action::EnablePerf,
        ),
        None => {}
    }
}

/// Per-list cap on generated entries (branches, tags…): the palette stays quick to filter.
const MAX_LIST_COMMANDS: usize = 300;

fn repo_commands(
    repo: &RepoContext,
    add: &mut impl FnMut(String, Option<KeyboardShortcut>, Action),
) {
    if repo.view != View::Changes {
        add(
            "Show Changes".into(),
            Some(shortcut::SHOW_CHANGES),
            Action::ShowChanges,
        );
    }
    if repo.view != View::History {
        add(
            "Show History".into(),
            Some(shortcut::SHOW_HISTORY),
            Action::ShowHistory,
        );
    }
    if repo.view == View::History && repo.has_commits {
        add(
            "Select Next Commit".into(),
            Some(shortcut::NEXT_FILE),
            Action::SelectNextCommit,
        );
        add(
            "Select Previous Commit".into(),
            Some(shortcut::PREVIOUS_FILE),
            Action::SelectPreviousCommit,
        );
    }

    rewrite_commands(repo, add);

    // The network.
    if repo.network_running {
        add(
            "Cancel Network Operation".into(),
            None,
            Action::CancelNetwork,
        );
    } else if !repo.remotes.is_empty() {
        add(
            "Fetch All Remotes".into(),
            Some(shortcut::FETCH),
            Action::Fetch,
        );
        if repo.remotes.len() > 1 {
            for remote in &repo.remotes {
                add(
                    format!("Fetch Remote: {remote}"),
                    None,
                    Action::FetchRemote(remote.clone()),
                );
            }
        }
        if !repo.bare && repo.branch.is_some() {
            if repo.upstream.is_some() {
                add(
                    format!("Pull ({})", repo.pull_mode.label()),
                    Some(shortcut::PULL),
                    Action::Pull,
                );
            }
            if repo.has_commits {
                add(
                    if repo.upstream.is_some() {
                        "Push"
                    } else {
                        "Push: Publish Branch"
                    }
                    .into(),
                    Some(shortcut::PUSH),
                    Action::Push,
                );
                if repo.upstream.is_some() {
                    add("Force Push (With Lease)…".into(), None, Action::ForcePush);
                }
            }
        }
    }
    for mode in PullMode::ALL {
        if mode != repo.pull_mode {
            add(
                format!("Pull Mode: {}", mode.label()),
                None,
                Action::SetPullMode(mode),
            );
        }
    }

    // Branches. `at: None` means "the selected commit in the history, else HEAD".
    add("Branches…".into(), None, Action::ShowBranches);
    if repo.has_commits {
        let at = match &repo.selected_commit {
            Some((_, short)) if repo.view == View::History => format!(" at {short}"),
            _ => String::new(),
        };
        add(
            format!("New Branch{at}…"),
            Some(shortcut::NEW_BRANCH),
            Action::NewBranch { at: None },
        );
        add(format!("New Tag{at}…"), None, Action::NewTag { at: None });
    }
    if !repo.bare {
        for name in repo.local_branches.iter().take(MAX_LIST_COMMANDS) {
            add(
                format!("Switch to Branch: {name}"),
                None,
                Action::SwitchBranch(name.clone()),
            );
        }
        if let (Some(current), None) = (&repo.branch, repo.operation) {
            for name in repo
                .local_branches
                .iter()
                .filter(|name| *name != current)
                .take(MAX_LIST_COMMANDS)
            {
                add(
                    format!("Merge {name} into {current}"),
                    None,
                    Action::MergeIntoCurrent(name.clone()),
                );
            }
        }
        for name in repo.remote_branches.iter().take(MAX_LIST_COMMANDS) {
            add(
                format!("Check Out Remote Branch: {name}"),
                None,
                Action::CheckoutRemoteBranch(name.clone()),
            );
        }
        if let (Some((oid, short)), View::History) = (&repo.selected_commit, repo.view) {
            add(
                format!("Check Out Commit {short} (Detached)"),
                None,
                Action::CheckoutCommit(oid.clone()),
            );
        }
    }
    if let Some(branch) = &repo.branch {
        if repo.has_commits {
            add(
                format!("Rename Branch: {branch}…"),
                None,
                Action::RenameBranch(branch.clone()),
            );
        }
    }
    for name in repo.local_branches.iter().take(MAX_LIST_COMMANDS) {
        add(
            format!("Delete Branch: {name}…"),
            None,
            Action::DeleteBranch(name.clone()),
        );
    }
    for name in repo.tags.iter().take(MAX_LIST_COMMANDS) {
        add(
            format!("Delete Tag: {name}…"),
            None,
            Action::DeleteTag(name.clone()),
        );
        if !repo.remotes.is_empty() && !repo.network_running {
            add(
                format!("Push Tag: {name}"),
                None,
                Action::PushTag(name.clone()),
            );
        }
    }

    // Remotes.
    add("Add Remote…".into(), None, Action::AddRemote);
    for remote in &repo.remotes {
        add(
            format!("Remove Remote: {remote}…"),
            None,
            Action::RemoveRemote(remote.clone()),
        );
    }

    // Stash.
    if !repo.bare {
        if repo.dirty {
            add(
                "Stash Changes…".into(),
                Some(shortcut::STASH),
                Action::Stash,
            );
            add("Stash Selected Files…".into(), None, Action::StashSelected);
        }
        if repo.staged {
            add("Stash Staged Changes".into(), None, Action::StashStaged);
        }
        for (index, message) in repo.stashes.iter().enumerate().take(MAX_LIST_COMMANDS) {
            add(
                format!("Apply Stash: {message}"),
                None,
                Action::ApplyStash(index),
            );
            add(
                format!("Pop Stash: {message}"),
                None,
                Action::PopStash(index),
            );
            add(
                format!("Drop Stash: {message}…"),
                None,
                Action::DropStash(index),
            );
        }
    }

    tree_commands(repo, add);

    // Link-outs.
    if let Some(forge) = repo.forge {
        let site = forge.label();
        if let (Some((oid, short)), View::History) = (&repo.selected_commit, repo.view) {
            add(
                format!("Open Commit {short} on {site}"),
                None,
                Action::OpenCommitOnForge(oid.clone()),
            );
        }
        if let Some(branch) = &repo.branch {
            add(
                format!("Open Branch {branch} on {site}"),
                None,
                Action::OpenBranchOnForge(branch.clone()),
            );
            let request = match forge {
                ForgeKind::GitLab => "Merge Request",
                _ => "Pull Request",
            };
            add(
                format!("Create {request} for {branch}"),
                None,
                Action::CreatePullRequest(branch.clone()),
            );
        }
    }
}

/// Worktrees, submodules and Git LFS (M4).
fn tree_commands(
    repo: &RepoContext,
    add: &mut impl FnMut(String, Option<KeyboardShortcut>, Action),
) {
    if repo.bare {
        return;
    }
    add(
        "Add Worktree…".into(),
        None,
        Action::AddWorktree { branch: None },
    );
    for (name, path) in repo.worktrees.iter().take(MAX_LIST_COMMANDS) {
        add(
            format!("Open Worktree: {name}  —  {}", path.display()),
            None,
            Action::OpenPath(path.clone()),
        );
        add(
            format!("Remove Worktree: {name}…"),
            None,
            Action::RemoveWorktree(path.clone()),
        );
    }
    if repo.prunable_worktrees {
        add(
            "Forget Worktrees Whose Folders Are Gone".into(),
            None,
            Action::PruneWorktrees,
        );
    }

    if !repo.submodules.is_empty() {
        if !repo.network_running {
            add(
                "Update All Submodules".into(),
                None,
                Action::UpdateSubmodules(None),
            );
        }
        add(
            "Sync All Submodule URLs".into(),
            None,
            Action::SyncSubmodules(None),
        );
        for (path, populated) in repo.submodules.iter().take(MAX_LIST_COMMANDS) {
            if *populated {
                add(
                    format!("Open Submodule: {path}"),
                    None,
                    Action::OpenSubmodule(path.clone()),
                );
            }
            if !repo.network_running {
                add(
                    format!("Update Submodule: {path}"),
                    None,
                    Action::UpdateSubmodules(Some(path.clone())),
                );
            }
        }
    }

    add("LFS: Track Files…".into(), None, Action::TrackWithLfs);
    if !repo.lfs {
        return;
    }
    add("LFS: Refresh Locks".into(), None, Action::RefreshLocks);
    if !repo.network_running && !repo.remotes.is_empty() {
        add("LFS: Download Files (Pull)".into(), None, Action::LfsPull);
    }
    if let Some((path, None)) = &repo.current_file {
        let name = path.rsplit('/').next().unwrap_or(path);
        if !repo.lfs_locks.iter().any(|lock| lock.path == *path) {
            add(
                format!("LFS: Lock {name}"),
                None,
                Action::LockFile(path.clone()),
            );
        }
    }
    for lock in repo.lfs_locks.iter().take(MAX_LIST_COMMANDS) {
        if lock.ours != Some(false) {
            add(
                format!("LFS: Unlock {}", lock.path),
                None,
                Action::UnlockFile {
                    id: lock.id.clone(),
                    path: lock.path.clone(),
                },
            );
        }
        if lock.ours != Some(true) {
            add(
                format!(
                    "LFS: Force Unlock {} (Locked by {})…",
                    lock.path, lock.owner
                ),
                None,
                Action::ForceUnlock {
                    id: lock.id.clone(),
                    path: lock.path.clone(),
                    owner: lock.owner.clone(),
                },
            );
        }
    }
}

/// Rewriting history, search, blame, conflicts and external tools (M3).
fn rewrite_commands(
    repo: &RepoContext,
    add: &mut impl FnMut(String, Option<KeyboardShortcut>, Action),
) {
    if let Some(kind) = repo.operation {
        let label = kind.label();
        if kind.can_continue() {
            add(format!("Continue {label}"), None, Action::ContinueOperation);
        }
        if kind.can_skip() {
            add(
                format!("{label}: Skip This Step"),
                None,
                Action::SkipOperation,
            );
        }
        add(format!("Abort {label}…"), None, Action::AbortOperation);
    }
    if let Some(title) = &repo.undo {
        add(
            format!("Undo: {title}"),
            Some(shortcut::UNDO_OPERATION),
            Action::UndoLastOperation,
        );
    }

    if repo.has_commits {
        add(
            "Search Commits…".into(),
            Some(shortcut::SEARCH),
            Action::FindCommits,
        );
        add(
            "Show Reflog (Where HEAD Has Been)".into(),
            Some(shortcut::REFLOG),
            Action::ShowReflog,
        );
    }
    if let Some(title) = &repo.filter {
        add(
            format!("Back to Full History ({title})"),
            None,
            Action::ClearFilter,
        );
    }
    if repo.has_commits {
        add(
            if repo.all_branches {
                "History: Show Only the Current Branch".into()
            } else {
                "History: Show All Branches".into()
            },
            None,
            Action::SetHistoryAllBranches(!repo.all_branches),
        );
    }
    if repo.commit_file_open {
        add("Back to the Commit".into(), None, Action::CloseCommitFile);
    }
    if let Some((path, rev)) = &repo.current_file {
        let name = path.rsplit('/').next().unwrap_or(path);
        add(
            format!("File History: {name}"),
            Some(shortcut::FILE_HISTORY),
            Action::FileHistoryCurrentFile,
        );
        add(
            match rev {
                Some(_) => format!("Blame: {name} (at This Commit)"),
                None => format!("Blame: {name}"),
            },
            Some(shortcut::BLAME),
            Action::BlameCurrentFile,
        );
        if repo.diff_tool {
            add(
                format!("Open {name} in External Diff Tool"),
                None,
                Action::OpenDiffTool,
            );
        }
    }
    if let Some(path) = &repo.conflicted_file {
        let name = path.rsplit('/').next().unwrap_or(path);
        add(
            format!("Resolve Conflicts in {name}…"),
            None,
            Action::ResolveConflicts(path.clone()),
        );
        add(
            format!("Resolve {name}: Take Ours (Current Branch)"),
            None,
            Action::TakeSide {
                path: path.clone(),
                side: Side::Ours,
            },
        );
        add(
            format!("Resolve {name}: Take Theirs (Incoming)"),
            None,
            Action::TakeSide {
                path: path.clone(),
                side: Side::Theirs,
            },
        );
        if repo.merge_tool {
            add(
                format!("Open {name} in External Merge Tool"),
                None,
                Action::OpenMergeTool(path.clone()),
            );
        }
    }

    // The selected commit of the history.
    if let (Some((oid, short)), View::History, false, None) =
        (&repo.selected_commit, repo.view, repo.bare, repo.operation)
    {
        add(
            format!("Cherry-Pick Commit {short}"),
            None,
            Action::CherryPick(oid.clone()),
        );
        add(
            format!("Revert Commit {short}"),
            None,
            Action::Revert(oid.clone()),
        );
        add(
            format!(
                "Merge Commit {short} into {}",
                repo.branch.as_deref().unwrap_or("HEAD")
            ),
            None,
            Action::MergeIntoCurrent(oid.clone()),
        );
        let branch = repo.branch.as_deref().unwrap_or("HEAD");
        for (mode, label) in [
            (ResetMode::Soft, "Soft (Keep Changes Staged)"),
            (ResetMode::Mixed, "Mixed (Keep Changes Unstaged)"),
            (ResetMode::Hard, "Hard (Discard Changes)…"),
        ] {
            add(
                format!("Reset {branch} to {short}: {label}"),
                None,
                Action::ResetTo {
                    oid: oid.clone(),
                    mode,
                },
            );
        }
        if repo.branch.is_some() {
            add(
                format!("Rebase {branch} onto {short}…"),
                None,
                Action::RebaseOnto(oid.clone()),
            );
            add(
                format!("Interactive Rebase {branch} From {short}…"),
                None,
                Action::InteractiveRebase {
                    base: Some(oid.clone()),
                },
            );
        }
    }

    add(
        "Add External Diff / Merge Tool…".into(),
        None,
        Action::AddExternalTool,
    );
    for (index, label) in repo.external_tools.iter().enumerate() {
        add(
            format!("Remove External Tool: {label}"),
            None,
            Action::RemoveExternalTool(index),
        );
    }
}

fn selected_files(count: usize) -> String {
    if count == 1 {
        "Selected File".to_owned()
    } else {
        format!("{count} Selected Files")
    }
}

/// Global shortcuts, most specific first (egui matches shortcuts logically, so `Ctrl+Shift+Tab`
/// must be tested before `Ctrl+Tab`).
pub const GLOBAL_SHORTCUTS: &[(KeyboardShortcut, Action)] = &[
    (shortcut::PREVIOUS_TAB, Action::PreviousTab),
    (shortcut::NEXT_TAB, Action::NextTab),
    (shortcut::REVEAL, Action::RevealInExplorer),
    (shortcut::COPY_PATH, Action::CopyRepositoryPath),
    (shortcut::STAGE_ALL, Action::StageAll),
    (shortcut::UNSTAGE_ALL, Action::UnstageAll),
    (shortcut::COMMIT, Action::Commit),
    (shortcut::FOCUS_MESSAGE, Action::FocusCommitMessage),
    (shortcut::FETCH, Action::Fetch),
    (shortcut::PULL, Action::Pull),
    // Before Ctrl+P: shortcuts match logically, so Ctrl+P would also match Ctrl+Shift+P.
    (shortcut::PUSH, Action::Push),
    (shortcut::NEW_BRANCH, Action::NewBranch { at: None }),
    (shortcut::STASH, Action::Stash),
    // Ctrl+Shift+… before Ctrl+… of the same key (Ctrl+Shift+B before the sidebar's Ctrl+B).
    (shortcut::UNDO_OPERATION, Action::UndoLastOperation),
    (shortcut::BLAME, Action::BlameCurrentFile),
    (shortcut::FILE_HISTORY, Action::FileHistoryCurrentFile),
    (shortcut::REFLOG, Action::ShowReflog),
    (shortcut::SEARCH, Action::FindCommits),
    (shortcut::SHOW_CHANGES, Action::ShowChanges),
    (shortcut::SHOW_HISTORY, Action::ShowHistory),
    (shortcut::PALETTE, Action::ShowPalette),
    (shortcut::PALETTE_ALT, Action::ShowPalette),
    (shortcut::OPEN, Action::ShowAddRepository(None)),
    (shortcut::CLOSE_TAB, Action::CloseActiveTab),
    (shortcut::REFRESH, Action::Refresh),
    (shortcut::SIDEBAR, Action::ToggleSidebar),
    (shortcut::COMMAND_LOG, Action::ToggleCommandLog),
    (shortcut::SETTINGS, Action::ShowSettings),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn context<'a>(tabs: &'a [String], recent: &'a [PathBuf]) -> CommandContext<'a> {
        CommandContext {
            tabs,
            active_tab: if tabs.is_empty() { None } else { Some(0) },
            repo_ready: !tabs.is_empty(),
            work: None,
            repo: None,
            recent,
            theme: ThemeMode::System,
            sidebar_visible: true,
            command_log_visible: false,
            command_log_empty: true,
            ui_scale: 1.0,
            code_font: CodeFont::CascadiaMono,
            code_font_size: 13.0,
            windows_accent: true,
            density: Density::Comfortable,
            auto_fetch: true,
            custom_git: false,
            open_in: &[],
        }
    }

    /// A working copy with something to do for every command.
    fn busy_work() -> WorkContext {
        WorkContext {
            staged: 2,
            unstaged: 3,
            selection: Some((Area::Unstaged, 2)),
            has_current: true,
            diff_area: Some(Area::Unstaged),
            hunks: 2,
            selected_lines: 3,
            can_commit: true,
            can_amend: true,
            amend: false,
            undo: Some(("notes.txt".into(), false)),
            perf: Some(PerfConfig::default()),
            image_mode: Some(ImageMode::SideBySide),
        }
    }

    /// A repository with branches, remotes and stashes, viewing the history.
    fn busy_repo() -> RepoContext {
        RepoContext {
            view: View::History,
            bare: false,
            branch: Some("main".into()),
            has_commits: true,
            upstream: Some("origin/main".into()),
            local_branches: vec!["topic".into()],
            remote_branches: vec!["origin/main".into(), "origin/topic".into()],
            tags: vec!["v1.0".into()],
            remotes: vec!["origin".into(), "fork".into()],
            stashes: vec!["On main: wip".into()],
            selected_commit: Some(("4659738f".repeat(5), "4659738".into())),
            network_running: false,
            pull_mode: PullMode::FastForwardOnly,
            all_branches: true,
            commit_file_open: false,
            forge: Some(ForgeKind::GitHub),
            dirty: true,
            staged: true,
            operation: None,
            undo: Some("Cherry-picked 4659738".into()),
            filter: None,
            current_file: Some(("src/main.rs".into(), None)),
            conflicted_file: None,
            merge_tool: false,
            diff_tool: true,
            external_tools: vec!["bcomp (all files)".into()],
            worktrees: vec![("game-hotfix".into(), PathBuf::from(r"D:\game-hotfix"))],
            prunable_worktrees: false,
            submodules: vec![("libs/engine".into(), true), ("libs/sdk".into(), false)],
            lfs: true,
            lfs_locks: vec![
                LfsLock {
                    id: "1".into(),
                    path: "Content/Hero.uasset".into(),
                    owner: "Ada".into(),
                    locked_at: String::new(),
                    ours: Some(false),
                },
                LfsLock {
                    id: "2".into(),
                    path: "Content/Map.umap".into(),
                    owner: "Me".into(),
                    locked_at: String::new(),
                    ours: Some(true),
                },
            ],
        }
    }

    #[test]
    fn worktree_submodule_and_lfs_commands_follow_the_state() {
        let tabs = ["game".to_owned()];
        let repo = busy_repo();
        let mut cx = context(&tabs, &[]);
        cx.repo = Some(&repo);
        let list = commands(&cx);
        let all = titles(&list);
        for expected in [
            "Add Worktree…",
            r"Open Worktree: game-hotfix  —  D:\game-hotfix",
            "Remove Worktree: game-hotfix…",
            "Update All Submodules",
            "Sync All Submodule URLs",
            "Open Submodule: libs/engine",
            "Update Submodule: libs/sdk",
            "LFS: Track Files…",
            "LFS: Refresh Locks",
            "LFS: Download Files (Pull)",
            "LFS: Lock main.rs",
            "LFS: Unlock Content/Map.umap",
            "LFS: Force Unlock Content/Hero.uasset (Locked by Ada)…",
        ] {
            assert!(all.contains(&expected), "missing {expected}");
        }
        // Not checked out: nothing to open. Ours: no force; theirs: no plain unlock.
        assert!(!all.contains(&"Open Submodule: libs/sdk"));
        assert!(!all.contains(&"LFS: Unlock Content/Hero.uasset"));
        assert!(!all
            .iter()
            .any(|t| t.starts_with("LFS: Force Unlock Content/Map")));
        assert!(!all.contains(&"Forget Worktrees Whose Folders Are Gone"));

        // Without LFS only tracking is offered; a bare repository has none of it.
        let plain = RepoContext {
            lfs: false,
            lfs_locks: Vec::new(),
            submodules: Vec::new(),
            prunable_worktrees: true,
            ..busy_repo()
        };
        cx.repo = Some(&plain);
        let list = commands(&cx);
        let all = titles(&list);
        assert!(all.contains(&"LFS: Track Files…"));
        assert!(all.contains(&"Forget Worktrees Whose Folders Are Gone"));
        assert!(!all.iter().any(|t| t.starts_with("LFS: Refresh")));
        assert!(!all.iter().any(|t| t.contains("Submodule")));
        let bare = RepoContext {
            bare: true,
            ..busy_repo()
        };
        cx.repo = Some(&bare);
        let list = commands(&cx);
        let all = titles(&list);
        assert!(!all
            .iter()
            .any(|t| t.contains("Worktree") || t.starts_with("LFS")));
    }

    fn titles(commands: &[Command]) -> Vec<&str> {
        commands.iter().map(|c| c.title.as_str()).collect()
    }

    #[test]
    fn without_a_repository_only_global_commands_are_offered() {
        let list = commands(&context(&[], &[]));
        let titles = titles(&list);
        assert!(titles.contains(&"Add Repository…"));
        assert!(titles.contains(&"Quit Facet"));
        assert!(!titles.contains(&"Refresh"));
        assert!(!titles.contains(&"Close Tab"));
        assert!(!titles.contains(&"Next Tab"));
        assert!(!titles.contains(&"Clear Command Log"));
        assert!(!titles.contains(&"Commit"));
        assert!(!titles.contains(&"Write Commit Message"));
    }

    #[test]
    fn repository_commands_appear_when_a_repo_is_open() {
        let tabs = ["game".to_owned(), "tools".to_owned()];
        let recent = [PathBuf::from(r"D:\old")];
        let list = commands(&context(&tabs, &recent));
        let titles = titles(&list);
        for expected in [
            "Refresh",
            "Open in File Explorer",
            "Copy Repository Path",
            "Close Tab",
            "Next Tab",
            "Previous Tab",
            "Switch to Tab: tools",
            r"Open Recent: old  —  D:\old",
        ] {
            assert!(titles.contains(&expected), "missing {expected}");
        }
        assert!(
            !titles.contains(&"Switch to Tab: game"),
            "active tab is skipped"
        );
    }

    #[test]
    fn toggles_and_theme_reflect_current_state() {
        let mut cx = context(&[], &[]);
        cx.sidebar_visible = false;
        cx.command_log_visible = true;
        cx.command_log_empty = false;
        cx.theme = ThemeMode::Dark;
        let list = commands(&cx);
        let titles = titles(&list);
        assert!(titles.contains(&"Show Sidebar"));
        assert!(titles.contains(&"Hide Command Log"));
        assert!(titles.contains(&"Clear Command Log"));
        assert!(titles.contains(&"Theme: System"));
        assert!(titles.contains(&"Theme: Light"));
        assert!(!titles.contains(&"Theme: Dark"));
    }

    #[test]
    fn working_copy_commands_follow_the_state() {
        let tabs = ["game".to_owned()];
        let work = busy_work();
        let mut cx = context(&tabs, &[]);
        cx.work = Some(&work);
        let list = commands(&cx);
        let all = titles(&list);
        for expected in [
            "Commit",
            "Write Commit Message",
            "Commit: Amend Last Commit",
            "Stage All Changes",
            "Unstage All Changes",
            "Stage 2 Selected Files",
            "Discard 2 Selected Files…",
            "Discard All Changes…",
            "Undo Discard: notes.txt",
            "Stage 3 Selected Lines",
            "Discard 3 Selected Lines…",
            "Copy 3 Selected Lines",
            "Clear Line Selection",
            "Stage Hunk 1",
            "Discard Hunk 2…",
            "Select Next File",
            "Select All Files in List",
            "Copy File Path",
            "Image Diff: Swipe",
            "Image Diff: Onion Skin",
            "Repository: Enable File System Monitor and Untracked Cache",
        ] {
            assert!(all.contains(&expected), "missing {expected}");
        }
        assert!(!all.contains(&"Image Diff: Side by Side"));
        assert!(!all.contains(&"Unstage Hunk 1"));

        // The staged side offers the opposite operations and never a discard.
        let staged = WorkContext {
            selection: Some((Area::Staged, 1)),
            diff_area: Some(Area::Staged),
            selected_lines: 1,
            amend: true,
            undo: Some(("notes.txt".into(), true)),
            perf: Some(PerfConfig {
                fsmonitor: true,
                untracked_cache: true,
            }),
            image_mode: None,
            ..busy_work()
        };
        cx.work = Some(&staged);
        let list = commands(&cx);
        let all = titles(&list);
        for expected in [
            "Amend Last Commit",
            "Commit: Stop Amending",
            "Unstage Selected File",
            "Unstage Selected Line",
            "Unstage Hunk 2",
            "Redo Discard: notes.txt",
            "Repository: Disable File System Monitor and Untracked Cache",
        ] {
            assert!(all.contains(&expected), "missing {expected}");
        }
        assert!(!all.iter().any(|title| title.starts_with("Discard Hunk")));
        assert!(!all.iter().any(|title| title.starts_with("Image Diff")));

        // A clean working copy offers none of it.
        let clean = WorkContext::default();
        cx.work = Some(&clean);
        let list = commands(&cx);
        let all = titles(&list);
        assert!(all.contains(&"Write Commit Message"));
        for absent in [
            "Commit",
            "Stage All Changes",
            "Select Next File",
            "Copy File Path",
        ] {
            assert!(!all.contains(&absent), "unexpected {absent}");
        }
    }

    #[test]
    fn repository_commands_follow_the_state() {
        let tabs = ["game".to_owned()];
        let repo = busy_repo();
        let mut cx = context(&tabs, &[]);
        cx.repo = Some(&repo);
        let list = commands(&cx);
        let all = titles(&list);
        for expected in [
            "Show Changes",
            "Select Next Commit",
            "Fetch All Remotes",
            "Fetch Remote: fork",
            "Pull (Fast-Forward Only)",
            "Push",
            "Force Push (With Lease)…",
            "Pull Mode: Rebase",
            "New Branch at 4659738…",
            "New Tag at 4659738…",
            "Switch to Branch: topic",
            "Check Out Remote Branch: origin/topic",
            "Check Out Commit 4659738 (Detached)",
            "Rename Branch: main…",
            "Delete Branch: topic…",
            "Delete Tag: v1.0…",
            "Push Tag: v1.0",
            "Add Remote…",
            "Remove Remote: fork…",
            "Stash Changes…",
            "Stash Selected Files…",
            "Stash Staged Changes",
            "Apply Stash: On main: wip",
            "Pop Stash: On main: wip",
            "Drop Stash: On main: wip…",
            "Open Commit 4659738 on GitHub",
            "Open Branch main on GitHub",
            "Create Pull Request for main",
        ] {
            assert!(all.contains(&expected), "missing {expected}");
        }
        assert!(!all.contains(&"Show History"));
        assert!(!all.contains(&"Cancel Network Operation"));

        // While a network operation runs, only cancelling it is offered.
        let running = RepoContext {
            network_running: true,
            ..busy_repo()
        };
        cx.repo = Some(&running);
        let list = commands(&cx);
        let all = titles(&list);
        assert!(all.contains(&"Cancel Network Operation"));
        assert!(!all
            .iter()
            .any(|t| t.starts_with("Fetch") || t.starts_with("Push")));

        // A new branch without an upstream is published; an empty bare repository offers
        // neither switching nor stashing.
        let unpublished = RepoContext {
            upstream: None,
            view: View::Changes,
            ..busy_repo()
        };
        cx.repo = Some(&unpublished);
        let list = commands(&cx);
        let all = titles(&list);
        assert!(all.contains(&"Push: Publish Branch"));
        assert!(all.contains(&"Show History"));
        assert!(all.contains(&"New Branch…"));
        assert!(!all.iter().any(|t| t.starts_with("Pull (")));
        assert!(!all.iter().any(|t| t.starts_with("Check Out Commit")));

        let bare = RepoContext {
            bare: true,
            branch: None,
            has_commits: false,
            dirty: false,
            staged: false,
            ..busy_repo()
        };
        cx.repo = Some(&bare);
        let list = commands(&cx);
        let all = titles(&list);
        assert!(!all.iter().any(|t| t.starts_with("Switch to Branch")));
        assert!(!all.iter().any(|t| t.contains("Stash")));
        assert!(!all.iter().any(|t| t.starts_with("New Branch")));
    }

    #[test]
    fn history_rewriting_commands_follow_the_state() {
        let tabs = ["game".to_owned()];
        let repo = busy_repo();
        let mut cx = context(&tabs, &[]);
        cx.repo = Some(&repo);
        let list = commands(&cx);
        let all = titles(&list);
        for expected in [
            "Undo: Cherry-picked 4659738",
            "Search Commits…",
            "Show Reflog (Where HEAD Has Been)",
            "File History: main.rs",
            "Blame: main.rs",
            "Open main.rs in External Diff Tool",
            "Cherry-Pick Commit 4659738",
            "Revert Commit 4659738",
            "Reset main to 4659738: Soft (Keep Changes Staged)",
            "Reset main to 4659738: Hard (Discard Changes)…",
            "Rebase main onto 4659738…",
            "Interactive Rebase main From 4659738…",
            "Add External Diff / Merge Tool…",
            "Remove External Tool: bcomp (all files)",
        ] {
            assert!(all.contains(&expected), "missing {expected}");
        }
        assert!(!all.iter().any(|t| t.starts_with("Continue ")));
        assert!(!all.iter().any(|t| t.starts_with("Back to Full History")));

        // During a rebase: continue / skip / abort, and nothing that starts another rewrite.
        let rebasing = RepoContext {
            operation: Some(OperationKind::Rebase),
            conflicted_file: Some("src/lib.rs".into()),
            filter: Some("Message: fix".into()),
            ..busy_repo()
        };
        cx.repo = Some(&rebasing);
        let list = commands(&cx);
        let all = titles(&list);
        for expected in [
            "Continue Rebase",
            "Rebase: Skip This Step",
            "Abort Rebase…",
            "Resolve Conflicts in lib.rs…",
            "Resolve lib.rs: Take Ours (Current Branch)",
            "Resolve lib.rs: Take Theirs (Incoming)",
            "Back to Full History (Message: fix)",
        ] {
            assert!(all.contains(&expected), "missing {expected}");
        }
        assert!(!all.iter().any(|t| t.starts_with("Cherry-Pick")));
        assert!(!all.iter().any(|t| t.starts_with("Interactive Rebase")));
        // No merge tool is set up for it.
        assert!(!all.iter().any(|t| t.contains("External Merge Tool")));

        // A detached HEAD can be reset but not rebased; a merge cannot be skipped.
        let detached = RepoContext {
            branch: None,
            ..busy_repo()
        };
        cx.repo = Some(&detached);
        let list = commands(&cx);
        let all = titles(&list);
        assert!(all.contains(&"Reset HEAD to 4659738: Mixed (Keep Changes Unstaged)"));
        assert!(!all.iter().any(|t| t.contains("Rebase HEAD")));
        let merging = RepoContext {
            operation: Some(OperationKind::Merge),
            ..busy_repo()
        };
        cx.repo = Some(&merging);
        let list = commands(&cx);
        let all = titles(&list);
        assert!(all.contains(&"Continue Merge"));
        assert!(!all.iter().any(|t| t.contains("Skip This Step")));
    }

    #[test]
    fn every_global_shortcut_is_reachable_from_the_palette() {
        let tabs = ["a".to_owned(), "b".to_owned()];
        let work = busy_work();
        let history = busy_repo();
        let changes = RepoContext {
            view: View::Changes,
            ..busy_repo()
        };
        let mut cx = context(&tabs, &[]);
        cx.work = Some(&work);
        cx.repo = Some(&history);
        let mut list = commands(&cx);
        // The view switch offers the view that is not showing.
        cx.repo = Some(&changes);
        list.extend(commands(&cx));
        for (shortcut, action) in GLOBAL_SHORTCUTS {
            if *action == Action::ShowPalette {
                continue; // the palette does not list itself
            }
            let entry = list
                .iter()
                .find(|c| c.action == *action)
                .unwrap_or_else(|| panic!("{action:?} has a shortcut but no palette entry"));
            assert_eq!(entry.shortcut, Some(*shortcut), "{action:?}");
        }
    }

    #[test]
    fn settings_and_integrations_are_in_the_palette() {
        let none = commands(&context(&[], &[]));
        let all = titles(&none);
        for expected in [
            "Settings…",
            "Interface Size: 110%",
            "Code Font: Consolas",
            "Choose Git Executable…",
            "Add External Tool: UnityYAMLMerge…",
        ] {
            assert!(all.contains(&expected), "missing {expected:?}");
        }
        // The current choices are not offered again; nothing needs a repository yet.
        assert!(!all.contains(&"Interface Size: 100%"));
        assert!(!all.contains(&"Code Font: Cascadia Mono"));
        assert!(!all.contains(&"Use Git from PATH"));
        assert!(!all
            .iter()
            .any(|t| t.starts_with("Open in ") || t.contains(".gitignore")));

        let tabs = ["game".to_owned()];
        let work = busy_work();
        let targets = [OpenTarget::Terminal, OpenTarget::Rider];
        let mut cx = context(&tabs, &[]);
        cx.work = Some(&work);
        cx.open_in = &targets;
        cx.custom_git = true;
        cx.ui_scale = 1.25;
        let list = commands(&cx);
        let all = titles(&list);
        for expected in [
            "Open in Windows Terminal",
            "Open in Rider",
            "Add Unity Rules to .gitignore…",
            "Add Unreal Engine Rules to .gitignore…",
            "Use Git from PATH",
            "Interface Size: 100%",
        ] {
            assert!(all.contains(&expected), "missing {expected:?}");
        }
        assert!(!all.contains(&"Open in VS Code"));
        assert!(!all.contains(&"Interface Size: 125%"));
    }

    #[test]
    fn shortcuts_are_unique() {
        for (i, (a, _)) in GLOBAL_SHORTCUTS.iter().enumerate() {
            for (b, _) in &GLOBAL_SHORTCUTS[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
