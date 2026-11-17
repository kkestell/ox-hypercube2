//! Worktrees that sessions run in: creating a worktree's branch, adding the
//! worktree, undoing both when it cannot be added, and
//! removing the worktree and the branch when its session goes. A
//! [`Worktree`] is the daemon's live record of one, which publishes how its
//! branch compares with its comparison ref.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ox_protocol::{Comparison, ErrorBody, WorktreeInfo, code};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use crate::daemon::invalid_request;
use crate::store::SavedWorktree;

/// The variables that point git at one repository, as `git rev-parse
/// --local-env-vars` lists them. Git runs in the directory it is given, so
/// none of them pass through.
pub(crate) const LOCAL_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// An argument to git.
pub type Arg<'a> = &'a (dyn AsRef<OsStr> + Sync);

/// Runs git for one operation, with the environment of the client that asked
/// for it.
#[derive(Clone)]
pub struct Git {
    env: BTreeMap<String, String>,
    /// Arguments that come before every command's own.
    prefix: Vec<OsString>,
}

/// A git command that failed, with git's error output.
#[derive(Debug)]
pub struct GitError {
    args: Vec<String>,
    stderr: String,
}

impl GitError {
    /// Git's error output, trimmed.
    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "git {} failed: {}", self.args.join(" "), self.stderr)
    }
}

impl From<GitError> for ErrorBody {
    fn from(error: GitError) -> Self {
        Self {
            code: code::GIT,
            message: error.to_string(),
        }
    }
}

impl Git {
    /// Git with `env` over the daemon's environment, as the client's agent
    /// gets it, and `git` found on that environment's `PATH`.
    pub fn for_client(env: BTreeMap<String, String>) -> Self {
        Self {
            env,
            prefix: Vec::new(),
        }
    }

    /// Git with the daemon's own environment, for work no client asked for.
    pub fn own() -> Self {
        Self::for_client(BTreeMap::new())
    }

    /// `git -C dir args`, with this environment, stdin null, and its output
    /// piped.
    pub fn command(&self, dir: &Path, args: &[Arg<'_>]) -> Command {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(dir)
            .args(&self.prefix)
            .args(args.iter().map(AsRef::as_ref))
            .envs(&self.env)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in LOCAL_ENV_VARS {
            command.env_remove(var);
        }
        command
    }

    /// Runs `git -C dir args` and returns its output. Callers put
    /// `--end-of-options` before names and revisions a user supplied.
    pub async fn run(&self, dir: &Path, args: &[Arg<'_>]) -> Result<Vec<u8>, GitError> {
        self.run_with_input(dir, args, None).await
    }

    async fn run_with_input(
        &self,
        dir: &Path,
        args: &[Arg<'_>],
        input: Option<&str>,
    ) -> Result<Vec<u8>, GitError> {
        let failed = |stderr: String| GitError {
            args: args
                .iter()
                .map(|arg| arg.as_ref().to_string_lossy().into_owned())
                .collect(),
            stderr,
        };
        let mut command = self.command(dir, args);
        let output = async {
            if let Some(input) = input {
                let mut child = command.stdin(Stdio::piped()).spawn()?;
                let mut stdin = child.stdin.take().expect("git stdin is piped");
                let write = stdin.write_all(input.as_bytes()).await;
                drop(stdin);
                let output = child.wait_with_output().await?;
                if output.status.success() {
                    write?;
                }
                Ok(output)
            } else {
                command.output().await
            }
        }
        .await
        .map_err(|error| failed(error.to_string()))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(failed(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }

    /// Like `run`, with the output as text without its final newline.
    pub(crate) async fn text(&self, dir: &Path, args: &[Arg<'_>]) -> Result<String, GitError> {
        let output = self.run(dir, args).await?;
        let text = String::from_utf8_lossy(&output);
        Ok(text.strip_suffix('\n').unwrap_or(&text).to_owned())
    }

    /// The commit `rev` names, if it names one.
    pub(crate) async fn commit(&self, dir: &Path, rev: &str) -> Option<String> {
        let rev = format!("{rev}^{{commit}}");
        self.text(
            dir,
            &[
                &"rev-parse",
                &"--verify",
                &"--quiet",
                &"--end-of-options",
                &rev,
            ],
        )
        .await
        .ok()
    }
}

/// A repository named by its common Git directory. Git runs in that
/// directory with `--git-dir` set to it, so it never discovers another
/// repository, whatever has become of the checkouts.
pub(crate) struct Repository<'a> {
    git: Git,
    common_dir: &'a Path,
}

impl<'a> Repository<'a> {
    pub(crate) fn new(git: &Git, common_dir: &'a Path) -> Self {
        let mut git_dir = OsString::from("--git-dir=");
        git_dir.push(common_dir);
        let mut git = git.clone();
        git.prefix.push(git_dir);
        Self { git, common_dir }
    }

    pub(crate) async fn run(&self, args: &[Arg<'_>]) -> Result<Vec<u8>, GitError> {
        self.git.run(self.common_dir, args).await
    }

    pub(crate) async fn run_with_input(
        &self,
        args: &[Arg<'_>],
        input: Option<&str>,
    ) -> Result<Vec<u8>, GitError> {
        self.git.run_with_input(self.common_dir, args, input).await
    }

    pub(crate) async fn text(&self, args: &[Arg<'_>]) -> Result<String, GitError> {
        self.git.text(self.common_dir, args).await
    }

    /// The commit `rev` names, if it names one.
    pub(crate) async fn commit(&self, rev: &str) -> Option<String> {
        self.git.commit(self.common_dir, rev).await
    }

    /// Whether git finds the repository.
    async fn found(&self) -> bool {
        self.run(&[&"rev-parse", &"--git-dir"]).await.is_ok()
    }
}

/// A worktree to add for a new session, resolved and checked.
#[derive(Debug)]
pub struct WorktreePlan {
    id: String,
    /// The top level of the workspace's checkout, where git runs.
    top: PathBuf,
    /// The repository's absolute common Git directory.
    common_dir: PathBuf,
    /// The worktree's top level.
    path: PathBuf,
    /// The session's directory: `path`, or the subdirectory of it that the
    /// workspace is of its checkout.
    pub cwd: PathBuf,
    branch: String,
    /// Whether the branch is new, created by `create_branch`, rather than an
    /// existing local branch.
    pub created_branch: bool,
    /// The commit the worktree starts at.
    base: String,
    /// The ref the branch is compared with.
    compare: Option<String>,
}

impl WorktreePlan {
    /// The worktree as the daemon saves it.
    pub fn saved(&self) -> SavedWorktree {
        SavedWorktree {
            id: self.id.clone(),
            path: self.path.clone(),
            branch: self.branch.clone(),
            base: self.base.clone(),
            created_branch: self.created_branch,
            compare: self.compare.clone(),
            common_dir: self.common_dir.clone(),
        }
    }
}

/// Resolves worktree `worktree_id` of `workspace`, on local branch `branch`
/// at its tip, or on a new branch named for the worktree that starts at
/// `HEAD`, compared with `compare`. Refuses, having changed nothing, a
/// workspace outside a Git working tree, a repository with no commits, a
/// `branch` that is not a local branch or is checked out in a worktree, a
/// comparison ref that is not a commit, or a worktree directory in
/// `worktrees_dir` that exists already.
pub async fn plan(
    git: &Git,
    workspace: &Path,
    compare: Option<String>,
    branch: Option<String>,
    worktrees_dir: &Path,
    worktree_id: &str,
) -> Result<WorktreePlan, ErrorBody> {
    let output = git
        .run(
            workspace,
            &[
                &"rev-parse",
                &"--show-toplevel",
                &"--show-prefix",
                &"--path-format=absolute",
                &"--git-common-dir",
            ],
        )
        .await
        .map_err(|error| not_a_working_tree(workspace, error.stderr()))?;
    let mut lines = output.split(|&byte| byte == b'\n');
    let top = PathBuf::from(OsStr::from_bytes(lines.next().unwrap_or_default()));
    let prefix = lines.next().unwrap_or_default();
    let prefix = PathBuf::from(OsStr::from_bytes(
        prefix.strip_suffix(b"/").unwrap_or(prefix),
    ));
    let common_dir = PathBuf::from(OsStr::from_bytes(lines.next().unwrap_or_default()));
    let name = top
        .file_name()
        .ok_or_else(|| not_a_working_tree(workspace, ""))?;

    let id8 = &worktree_id[..8];
    let mut directory = OsString::from(name);
    directory.push(format!("-{id8}"));
    let path = worktrees_dir.join(directory);
    if path.symlink_metadata().is_ok() {
        return Err(invalid_request(format!(
            "{} already exists",
            path.display()
        )));
    }

    let created_branch = branch.is_none();
    let (branch, base) = match branch {
        Some(branch) => {
            let base = git
                .commit(&top, &format!("refs/heads/{branch}"))
                .await
                .ok_or_else(|| invalid_request(format!("{branch} is not a local branch")))?;
            if let Some(checkout) =
                checked_out_at(&Repository::new(git, &common_dir), &branch).await?
            {
                return Err(invalid_request(format!(
                    "{branch} is checked out at {checkout}"
                )));
            }
            (branch, base)
        }
        None => {
            let base = git
                .commit(&top, "HEAD")
                .await
                .ok_or_else(|| invalid_request("the repository has no commits".into()))?;
            (format!("ox/{id8}"), base)
        }
    };
    if let Some(compare) = &compare
        && git.commit(&top, compare).await.is_none()
    {
        return Err(invalid_request(format!("{compare} is not a commit")));
    }
    if !prefix.as_os_str().is_empty() {
        let mut object = OsString::from(format!("{base}:"));
        object.push(prefix.as_os_str());
        if git.run(&top, &[&"cat-file", &"-e", &object]).await.is_err() {
            return Err(invalid_request(format!(
                "{} does not exist at {base}",
                prefix.display()
            )));
        }
    }
    Ok(WorktreePlan {
        id: worktree_id.to_owned(),
        cwd: path.join(&prefix),
        top,
        common_dir,
        path,
        branch,
        created_branch,
        base,
        compare,
    })
}

/// The refusal of a workspace that `git rev-parse --show-toplevel` finds in
/// no working tree: one outside any repository, in a bare repository, inside
/// a `.git` directory, or in a repository git rejects. `reason` is git's
/// error output, if it gave any.
pub fn not_a_working_tree(workspace: &Path, reason: &str) -> ErrorBody {
    let refusal = format!("{} is not in a Git working tree", workspace.display());
    invalid_request(if reason.is_empty() {
        refusal
    } else {
        format!("{refusal}: {reason}")
    })
}

/// The worktree that has local branch `branch` checked out, if any.
pub(crate) async fn checked_out_at(
    repository: &Repository<'_>,
    branch: &str,
) -> Result<Option<String>, GitError> {
    let worktrees = repository
        .text(&[&"worktree", &"list", &"--porcelain"])
        .await?;
    let checked_out = format!("branch refs/heads/{branch}");
    let mut worktree = None;
    for line in worktrees.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            worktree = Some(path);
        } else if line == checked_out {
            return Ok(worktree.map(String::from));
        }
    }
    Ok(None)
}

/// Creates the plan's branch at its base. Creation fails if a branch of that
/// name was created meanwhile.
pub async fn create_branch(git: &Git, plan: &WorktreePlan) -> Result<(), ErrorBody> {
    let branch = plan.branch.as_str();
    let base = plan.base.as_str();
    git.run(
        &plan.top,
        &[
            &"branch",
            &"--no-track",
            &"--end-of-options",
            &branch,
            &base,
        ],
    )
    .await
    .map(drop)
    .map_err(|error| invalid_request(error.stderr))
}

/// Adds the plan's worktree on its branch, which exists. A worktree that
/// cannot be added is undone with its branch, as `roll_back` does. Returns
/// why it could not be added, and what could not be undone.
pub async fn add(git: &Git, plan: &WorktreePlan) -> Result<(), String> {
    let Err(mut message) = add_worktree(git, plan).await else {
        return Ok(());
    };
    if let Err(error) = roll_back(git, plan, true).await {
        message = format!("{message}; undoing it failed: {error}");
    }
    Err(message)
}

async fn add_worktree(git: &Git, plan: &WorktreePlan) -> Result<(), String> {
    if let Some(dir) = plan.path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|error| format!("creating {}: {error}", dir.display()))?;
    }
    git.run(
        &plan.top,
        &[
            &"worktree",
            &"add",
            &"--end-of-options",
            &plan.path,
            &plan.branch,
        ],
    )
    .await
    .map(drop)
    .map_err(|error| error.to_string())
}

/// Adds the worktree again at its saved path, on its saved branch. Succeeds
/// without change when git already has it there.
async fn restore(git: &Git, worktree: &SavedWorktree) -> Result<(), ErrorBody> {
    let path = worktree.path.as_path();
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|error| ErrorBody {
                code: code::INVALID_STATE,
                message: format!("creating {}: {error}", dir.display()),
            })?;
    }
    let repository = Repository::new(git, &worktree.common_dir);
    if path.exists()
        && checked_out_at(&repository, &worktree.branch)
            .await?
            .is_some_and(|listed| is_listed_at(Path::new(&listed), path))
    {
        return Ok(());
    }
    repository
        .run(&[
            &"worktree",
            &"add",
            &"--end-of-options",
            &path,
            &worktree.branch,
        ])
        .await?;
    Ok(())
}

/// Undoes `create_branch`, and a failed `add` when `added` says it ran:
/// removes the worktree if git registered it, and deletes a branch the plan
/// created if it still points at its base and no worktree has it checked
/// out. Returns what could not be undone.
pub async fn roll_back(git: &Git, plan: &WorktreePlan, added: bool) -> Result<(), String> {
    let mut failures = Vec::new();
    if added
        && plan.path.symlink_metadata().is_ok()
        && let Err(error) = git
            .run(&plan.top, &[&"worktree", &"remove", &"--force", &plan.path])
            .await
    {
        failures.push(error.to_string());
    }
    // Another worktree may have checked the branch out meanwhile, which is
    // what made `add` fail.
    if plan.created_branch {
        match checked_out_at(&Repository::new(git, &plan.common_dir), &plan.branch).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = delete_branch(git, plan).await {
                    failures.push(error);
                }
            }
            Err(error) => failures.push(error.to_string()),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// Deletes the plan's branch if it still points at its base.
async fn delete_branch(git: &Git, plan: &WorktreePlan) -> Result<(), String> {
    let reference = format!("refs/heads/{}", plan.branch);
    let base = plan.base.as_str();
    git.run(&plan.top, &[&"update-ref", &"-d", &reference, &base])
        .await
        .map(drop)
        .map_err(|error| error.to_string())
}

/// What removing the worktree would lose: uncommitted changes, commits on no
/// branch, and commits of initialized submodules on no remote branch. A
/// worktree git can no longer use, because its repository is gone or no
/// longer knows it, cannot be checked, which is listed too. A worktree
/// directory that no longer exists has nothing to lose.
async fn unsaved(git: &Git, worktree: &SavedWorktree) -> Result<Vec<String>, GitError> {
    let path = worktree.path.as_path();
    if path.symlink_metadata().is_err() {
        return Ok(Vec::new());
    }
    if let Err(error) = usable(git, path).await {
        let reason = if Repository::new(git, &worktree.common_dir).found().await {
            format!(
                "git no longer recognizes {} as a worktree, so Ox cannot check it for unsaved \
                 work: {}",
                path.display(),
                error.stderr()
            )
        } else {
            format!(
                "the repository of {} no longer exists, so Ox cannot check it for unsaved work",
                path.display()
            )
        };
        return Ok(vec![reason]);
    }
    let mut unsaved = Vec::new();
    // Ignored files are not work to keep.
    let status = git
        .run(
            path,
            &[
                &"--no-optional-locks",
                &"status",
                &"--porcelain",
                &"--untracked-files=all",
                &"--ignore-submodules=none",
            ],
        )
        .await?;
    if !status.is_empty() {
        unsaved.push(format!("uncommitted changes in {}", path.display()));
    }
    let base = worktree.base.as_str();
    let detached = git
        .text(
            path,
            &[
                &"rev-list",
                &"--count",
                &"HEAD",
                &"--not",
                &base,
                &"--branches",
                &"--remotes",
                &"--tags",
            ],
        )
        .await?;
    if detached != "0" {
        unsaved.push(format!("commits on no branch in {}", path.display()));
    }
    if path.join(".gitmodules").exists() {
        let submodules = git
            .run(
                path,
                &[
                    &"submodule",
                    &"foreach",
                    &"--quiet",
                    &"--recursive",
                    &"printf '%s\\0' \"$displaypath\"",
                ],
            )
            .await?;
        for submodule in submodules.split(|&byte| byte == 0) {
            if submodule.is_empty() {
                continue;
            }
            let submodule = Path::new(OsStr::from_bytes(submodule));
            let unpushed = git
                .text(
                    &path.join(submodule),
                    &[
                        &"rev-list",
                        &"--count",
                        &"HEAD",
                        &"--branches",
                        &"--not",
                        &"--remotes",
                    ],
                )
                .await?;
            if unpushed != "0" {
                unsaved.push(format!(
                    "submodule {} has commits on no remote branch",
                    submodule.display()
                ));
            }
        }
    }
    Ok(unsaved)
}

/// The refusal to remove a worktree that would lose `unsaved`.
fn unsaved_work(unsaved: &[String]) -> ErrorBody {
    ErrorBody {
        code: code::UNSAVED_WORK,
        message: format!("deleting the worktree would lose {}", unsaved.join("; ")),
    }
}

/// Checks that git works in the worktree at `path`, which exists.
async fn usable(git: &Git, path: &Path) -> Result<(), GitError> {
    git.run(path, &[&"rev-parse", &"--git-dir"]).await.map(drop)
}

/// Removes the worktree, ignored files included. Unless `discard` is set, a
/// worktree with anything `unsaved` lists is refused with `UNSAVED_WORK` and
/// left as it is. A worktree git can no longer use is deleted as a
/// directory. A worktree that is gone from disk is unregistered, if its
/// repository still lists it.
async fn remove_worktree(
    git: &Git,
    worktree: &SavedWorktree,
    discard: bool,
) -> Result<(), ErrorBody> {
    let path = worktree.path.as_path();
    let repository = Repository::new(git, &worktree.common_dir);
    if path.symlink_metadata().is_ok() {
        if !discard {
            let unsaved = unsaved(git, worktree).await?;
            if !unsaved.is_empty() {
                return Err(unsaved_work(&unsaved));
            }
        }
        if usable(git, path).await.is_ok() {
            // The check above stands in for git's own, which also refuses
            // any worktree with initialized submodules.
            remove_registered(&repository, path).await?;
            return Ok(());
        }
        // The directory is the daemon's own, and git cannot use it.
        tokio::fs::remove_dir_all(path)
            .await
            .map_err(|error| ErrorBody {
                code: code::INVALID_STATE,
                message: format!("removing {}: {error}", path.display()),
            })?;
    }
    if repository.found().await && registered(&repository, path).await? {
        remove_registered(&repository, path).await?;
    }
    Ok(())
}

async fn remove_registered(repository: &Repository<'_>, path: &Path) -> Result<(), GitError> {
    repository
        .run(&[&"worktree", &"remove", &"--force", &path])
        .await
        .map(drop)
}

/// Whether the repository lists a worktree at `path`, which no longer
/// exists. Git lists worktrees by their real paths, so `path` is compared
/// with its parent resolved.
async fn registered(repository: &Repository<'_>, path: &Path) -> Result<bool, GitError> {
    let worktrees = repository
        .run(&[&"worktree", &"list", &"--porcelain", &"-z"])
        .await?;
    Ok(worktrees
        .split(|&byte| byte == 0)
        .filter_map(|field| field.strip_prefix(b"worktree "))
        .any(|listed| is_listed_at(Path::new(OsStr::from_bytes(listed)), path)))
}

/// Whether git's listing path `listed` names the worktree at `path`. Git
/// lists worktrees by their real paths, so `path` is also compared with its
/// parent resolved.
fn is_listed_at(listed: &Path, path: &Path) -> bool {
    listed == path
        || path
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .zip(path.file_name())
            .is_some_and(|(parent, name)| parent.join(name) == listed)
}

/// Applies the branch rule to the worktree's branch, in its repository.
/// Returns the branch when it is kept: a branch the daemon created is
/// deleted if every commit on it since the worktree's base is on another
/// branch, a remote branch or a tag, and no worktree has it checked out, and
/// kept otherwise. A branch the daemon cannot check, because its repository
/// is gone, is reported as kept. A branch that existed before the worktree,
/// and a branch that is gone, leave nothing to do.
async fn apply_branch_rule(
    git: &Git,
    worktree: &SavedWorktree,
) -> Result<Option<String>, GitError> {
    let SavedWorktree {
        branch,
        base,
        created_branch,
        ..
    } = worktree;
    if !created_branch {
        return Ok(None);
    }
    let repository = Repository::new(git, &worktree.common_dir);
    if !repository.found().await {
        return Ok(Some(branch.clone()));
    }
    let reference = format!("refs/heads/{branch}");
    let Some(tip) = repository.commit(&reference).await else {
        return Ok(None);
    };
    let exclude = format!("--exclude={branch}");
    let only_here = repository
        .text(&[
            &"rev-list",
            &"--count",
            &tip,
            &"--not",
            base,
            &exclude,
            &"--branches",
            &"--remotes",
            &"--tags",
        ])
        .await?;
    if only_here != "0" || checked_out_at(&repository, branch).await?.is_some() {
        return Ok(Some(branch.clone()));
    }
    // Deleting only at the tip just read keeps a commit made meanwhile.
    repository
        .run(&[&"update-ref", &"-d", &reference, &tip])
        .await?;
    Ok(None)
}

/// How the worktree's branch compares with its comparison ref, in its
/// repository: the commits on each, and how many commits each has that the
/// other lacks. Unset when the worktree has no comparison ref.
async fn compare(git: &Git, worktree: &SavedWorktree) -> Option<Comparison> {
    let compare = worktree.compare.as_deref()?;
    let branch = &worktree.branch;
    let repository = Repository::new(git, &worktree.common_dir);
    let failed = |error: String| Some(Comparison::Failed { error });
    let Some(tip) = repository.commit(&format!("refs/heads/{branch}")).await else {
        return failed(format!("branch {branch} is not a commit"));
    };
    let Some(compare_tip) = repository.commit(compare).await else {
        return failed(format!("{compare} is not a commit"));
    };
    let range = format!("{compare_tip}...{tip}");
    let counts = match repository
        .text(&[&"rev-list", &"--left-right", &"--count", &range])
        .await
    {
        Ok(counts) => counts,
        Err(error) => return failed(error.to_string()),
    };
    let mut counts = counts.split_whitespace().map(str::parse::<u64>);
    let (Some(Ok(behind)), Some(Ok(ahead))) = (counts.next(), counts.next()) else {
        return failed(format!("git rev-list printed no counts for {range}"));
    };
    Some(Comparison::Compared {
        tip,
        compare_tip,
        ahead,
        behind,
    })
}

/// A worktree the daemon added for a session: its saved record, and how its
/// branch compares with its comparison ref, which it computes when it is
/// created and again each time it is asked.
pub struct Worktree {
    pub id: String,
    saved: Mutex<SavedWorktree>,
    /// The comparison as last computed. Locked after `saved`.
    comparison: Mutex<Option<Comparison>>,
    /// Held while a comparison is computed and published, so that one that
    /// read git later is published later.
    comparing: tokio::sync::Mutex<()>,
    /// Asks the comparing task to compute the comparison again.
    recompare: Notify,
    comparer: Mutex<Option<JoinHandle<()>>>,
    /// Counts the changes to the worktree's `WorktreeInfo`. Changed under the
    /// `saved` lock.
    version: AtomicU64,
    /// The daemon's signal that the lists changed, sent when this worktree's
    /// `WorktreeInfo` changes.
    changed: watch::Sender<()>,
}

impl Worktree {
    /// The live worktree for `saved`, which starts computing its comparison.
    pub fn new(saved: SavedWorktree, changed: watch::Sender<()>) -> Arc<Self> {
        let worktree = Arc::new(Self {
            id: saved.id.clone(),
            saved: Mutex::new(saved),
            comparison: Mutex::new(None),
            comparing: tokio::sync::Mutex::new(()),
            recompare: Notify::new(),
            comparer: Mutex::new(None),
            version: AtomicU64::new(0),
            changed,
        });
        *worktree.comparer.lock().unwrap() =
            Some(tokio::spawn(worktree.clone().compare_when_asked()));
        worktree
    }

    /// Computes the comparison, then again each time `recompare` is
    /// notified. Notifications that arrive while it computes make one more
    /// computation.
    async fn compare_when_asked(self: Arc<Self>) {
        loop {
            self.compare().await;
            self.recompare.notified().await;
        }
    }

    pub fn info(&self) -> WorktreeInfo {
        self.info_of(&self.saved.lock().unwrap())
    }

    /// The worktree's `WorktreeInfo` and its version, which grows with each
    /// change.
    pub fn versioned_info(&self) -> (WorktreeInfo, u64) {
        let saved = self.saved.lock().unwrap();
        (self.info_of(&saved), self.version())
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn info_of(&self, saved: &SavedWorktree) -> WorktreeInfo {
        // A cleared ref has no comparison, whatever a comparison under way
        // stores.
        let comparison = saved
            .compare
            .as_ref()
            .and_then(|_| self.comparison.lock().unwrap().clone());
        WorktreeInfo {
            id: saved.id.clone(),
            path: saved.path.clone(),
            branch: saved.branch.clone(),
            base: saved.base.clone(),
            created_branch: saved.created_branch,
            compare: saved.compare.clone(),
            comparison,
        }
    }

    pub fn saved(&self) -> SavedWorktree {
        self.saved.lock().unwrap().clone()
    }

    /// Changes the worktree and, if its `WorktreeInfo` changed, signals it.
    fn update(&self, change: impl FnOnce(&mut SavedWorktree, &mut Option<Comparison>)) {
        let mut saved = self.saved.lock().unwrap();
        let before = self.info_of(&saved);
        change(&mut saved, &mut self.comparison.lock().unwrap());
        if self.info_of(&saved) != before {
            self.version.fetch_add(1, Ordering::Release);
            self.changed.send_replace(());
        }
    }

    /// Computes how the branch compares with the comparison ref and
    /// publishes it.
    pub async fn compare(&self) {
        let _comparing = self.comparing.lock().await;
        let comparison = compare(&Git::own(), &self.saved()).await;
        self.update(|_, current| *current = comparison);
    }

    /// Asks the comparing task to compute the comparison again.
    pub fn recompare(&self) {
        self.recompare.notify_one();
    }

    /// Removes the worktree once `stop` has stopped the agent running in it.
    /// Unless `discard` is set, a worktree with unsaved work is refused with
    /// `UNSAVED_WORK` before `stop` runs, and again if work appeared before
    /// the agent stopped.
    pub async fn remove(
        &self,
        git: &Git,
        discard: bool,
        stop: impl Future<Output = ()>,
    ) -> Result<(), ErrorBody> {
        let saved = self.saved();
        if !discard {
            let unsaved = unsaved(git, &saved).await?;
            if !unsaved.is_empty() {
                return Err(unsaved_work(&unsaved));
            }
        }
        stop.await;
        remove_worktree(git, &saved, discard).await
    }

    /// Adds the removed worktree again at its path, on its branch.
    pub async fn restore(&self, git: &Git) -> Result<(), ErrorBody> {
        restore(git, &self.saved()).await
    }

    /// Applies the branch rule to the worktree's branch, as
    /// [`apply_branch_rule`] describes, and returns the branch if it is kept.
    pub async fn apply_branch_rule(&self, git: &Git) -> Result<Option<String>, GitError> {
        apply_branch_rule(git, &self.saved()).await
    }

    /// Stops computing the comparison.
    pub fn close(&self) {
        let comparer = self.comparer.lock().unwrap().take();
        if let Some(comparer) = comparer {
            comparer.abort();
        }
    }
}
