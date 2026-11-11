//! The method an agent review follows, one markdown playbook per depth, and the first
//! message that starts a review. The first message only names the depth, the target
//! and the playbook's path; the method lives in the playbook.
//!
//! The text ships inside the binary and is written to the app's data directory on use:
//! a real folder the agent's shell can read, in dev and in the bundled app alike.
//! Adapted ideas, not files, from SpillwaveSolutions/pr-reviewer-skill (MIT) and
//! cloudflare/security-audit-skill (MIT).

use super::findings::{FindingsMode, LocalScope, ReviewMode};
use std::path::{Path, PathBuf};

const PLAYBOOK_DIR: &str = "review-playbooks";
/// The tool a reviewer hands its findings to; the playbook names it.
pub const SUBMIT_FINDINGS_TOOL: &str = "pandacode_submit_review_findings";
/// The tool the learning agent hands a project's review skill to.
pub const SUBMIT_SKILL_TOOL: &str = "pandacode_submit_review_skill";
pub const POST_FINDINGS_TOOL: &str = "pandacode_post_review_findings";

const CREDIT: &str = "<!-- Review playbook. Adapted ideas, not files, from\nSpillwaveSolutions/pr-reviewer-skill (MIT) and cloudflare/security-audit-skill (MIT), and from how\nCodeRabbit, Greptile, Qodo PR-Agent and Claude Code's /code-review describe their reviews. -->";
const SHARED: &str = include_str!("playbooks/shared.md");
const QUICK: &str = include_str!("playbooks/quick.md");
const DEEP: &str = include_str!("playbooks/deep.md");
const SECURITY: &str = include_str!("playbooks/security.md");
const LEARN: &str = include_str!("playbooks/learn-review-skill.md");

fn file_name(mode: ReviewMode) -> &'static str {
    match mode {
        ReviewMode::Quick => "quick-review.md",
        ReviewMode::Deep => "deep-review.md",
        ReviewMode::Security => "security-audit.md",
    }
}

/// The playbook text for one depth: its method, then the rules and report every depth shares.
pub fn playbook_markdown(mode: ReviewMode) -> String {
    let method = match mode {
        ReviewMode::Quick => QUICK,
        ReviewMode::Deep => DEEP,
        ReviewMode::Security => SECURITY,
    };
    let shared = SHARED.replace("{{SUBMIT_TOOL}}", SUBMIT_FINDINGS_TOOL).replace("{{POST_TOOL}}", POST_FINDINGS_TOOL).replace("{{CLI_FALLBACK}}", "");
    format!("{CREDIT}\n\n# {}\n\n{}\n\n{}\n", mode.label(), method.trim_end(), shared.trim_end())
}

/// The learning agent's method: how it reads a repository's history and what it hands over.
pub fn skill_playbook_markdown() -> String {
    LEARN.replace("{{SKILL_TOOL}}", SUBMIT_SKILL_TOOL)
}

/// Writes `content` when the file is missing or out of date, and returns its path. Reviews
/// start concurrently, so a changed file is swapped in by rename and no agent reads a
/// half-written one.
pub(crate) fn ensure_file(dir: &Path, name: &str, content: &str) -> std::io::Result<PathBuf> {
    let target = dir.join(name);
    if std::fs::read_to_string(&target).is_ok_and(|current| current == content) {
        return Ok(target);
    }
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!("{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&temporary, content)?;
    if let Err(error) = std::fs::rename(&temporary, &target) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(target)
}

pub fn ensure_playbook(data_dir: &Path, mode: ReviewMode) -> std::io::Result<PathBuf> {
    ensure_file(&data_dir.join(PLAYBOOK_DIR), file_name(mode), &playbook_markdown(mode))
}

pub fn ensure_skill_playbook(data_dir: &Path) -> std::io::Result<PathBuf> {
    ensure_file(&data_dir.join(PLAYBOOK_DIR), "learn-review-skill.md", &skill_playbook_markdown())
}

/// What a re-review is told: where the earlier findings are, and what to read again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReReview {
    pub previous_path: String,
    /// The commit the earlier review read and the one this one reads, where both are known.
    pub previous_head_sha: Option<String>,
    pub head_sha: Option<String>,
}

fn re_review_lines(re_review: &ReReview) -> Vec<String> {
    let scope = match (&re_review.previous_head_sha, &re_review.head_sha) {
        (Some(previous), Some(head)) if previous == head => format!("Nothing was pushed since that review read commit {previous}, so there is nothing new to review."),
        (Some(previous), Some(head)) => format!("That review read commit {previous}. Review only what changed since: `git diff {previous} {head}` (fetch the head first), and the code those changes reach."),
        _ => "Review the whole change again.".to_owned(),
    };
    vec![
        String::new(),
        format!("This is a re-review. The previous review's findings are in {} (see the playbook's Re-review section).", re_review.previous_path),
        scope,
        format!("Then check every previous finding against the code as it is now, and when you call {SUBMIT_FINDINGS_TOOL}, include each one still open again and pass previous with how many were fixed and how many are still open."),
    ]
}

/// The files a first message may point the reviewer at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptFiles {
    pub re_review: Option<ReReview>,
    /// The project's recently rated findings, when any were rated.
    pub rated_findings_path: Option<String>,
    /// The lessons of the project's review skill that are on, when it has any.
    pub review_skill_path: Option<String>,
}

fn file_lines(lines: &mut Vec<String>, focus: Option<&str>, files: &PromptFiles) {
    if let Some(focus) = focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        lines.extend([String::new(), format!("Focus on: {focus}")]);
    }
    if let Some(re_review) = &files.re_review {
        lines.extend(re_review_lines(re_review));
    }
    if let Some(path) = &files.rated_findings_path {
        lines.extend([String::new(), format!("How I rated findings of earlier reviews in this project is in {path} (see the playbook).")]);
    }
    if let Some(path) = &files.review_skill_path {
        lines.extend([String::new(), format!("What this project's history taught, to look for in this change, is in {path} (see the playbook's Project review skill section).")]);
    }
}

/// The pull request a review reads, as its host describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestFacts {
    /// `github`, `gitlab`, `forgejo`, `azure-devops`, `bitbucket`; anything else is unknown.
    pub provider: String,
    pub repository: String,
    pub url: String,
    pub number: u32,
    pub title: String,
    pub head_branch: String,
    pub base_branch: String,
}

fn host_label(provider: &str) -> &'static str {
    match provider {
        "github" => "GitHub",
        "gitlab" => "GitLab",
        "forgejo" => "Forgejo",
        "azure-devops" => "Azure DevOps",
        "bitbucket" => "Bitbucket",
        _ => "Unknown host",
    }
}

/// The ref a host publishes a change's head under, which also reaches a fork's branch.
pub fn host_head_ref(pr: &PullRequestFacts) -> String {
    match pr.provider.as_str() {
        "github" | "forgejo" => format!("pull/{}/head", pr.number),
        "gitlab" => format!("merge-requests/{}/head", pr.number),
        "azure-devops" => format!("pull/{}/merge", pr.number),
        _ => pr.head_branch.clone(),
    }
}

pub fn pull_request_title(mode: ReviewMode, pr: &PullRequestFacts) -> String {
    format!("{}: #{} {}", mode.title_prefix(), pr.number, pr.title)
}

/// Present when the findings will be posted as review comments: the user's style, and the
/// file of their own past review comments learned for the account they post as.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommentStyle {
    pub style: Option<String>,
    pub samples_path: Option<String>,
}

pub struct PullRequestPrompt<'a> {
    pub mode: ReviewMode,
    pub playbook_path: &'a str,
    pub pr: &'a PullRequestFacts,
    pub focus: Option<&'a str>,
    /// The head commit to review, which the findings are pinned to.
    pub head_sha: Option<&'a str>,
    /// Only a mode the user chose; otherwise the app decides when the findings arrive.
    pub findings_mode: Option<FindingsMode>,
    pub files: &'a PromptFiles,
    pub comments: Option<&'a CommentStyle>,
}

/// The first message of a review thread. Short on purpose; the method lives in the playbook.
pub fn pull_request_prompt(input: &PullRequestPrompt<'_>) -> String {
    let pr = input.pr;
    let mut lines = vec![
        format!("{} of pull request #{}: {}", input.mode.label(), pr.number, pr.title),
        pr.url.clone(),
        format!("{} {}, {} into {} (head ref {}).", host_label(&pr.provider), pr.repository, pr.head_branch, pr.base_branch, host_head_ref(pr)),
    ];
    if let Some(head) = input.head_sha {
        lines.push(format!("Review head commit {head}. If the pull request has moved past it, still review that commit: your findings are pinned to it."));
    }
    lines.extend([
        String::new(),
        format!("Read the review playbook at {} and follow it for this pull request. Do not change my working tree, and post nothing to the host unless I ask.", input.playbook_path),
    ]);
    file_lines(&mut lines, input.focus, input.files);
    if let Some(mode) = input.findings_mode {
        let mode = if mode == FindingsMode::Fix { "fix" } else { "comment" };
        lines.extend([String::new(), format!("When you call {SUBMIT_FINDINGS_TOOL}, pass mode \"{mode}\".")]);
    }
    if let Some(comments) = input.comments {
        lines.extend([
            String::new(),
            "I will post your findings to this pull request as my review comments, so write each finding's detail as the comment itself (see the playbook's Report section).".to_owned(),
        ]);
        if let Some(path) = &comments.samples_path {
            lines.extend([String::new(), format!("Write them the way I write reviews: my past review comments are in {path} (see the playbook).")]);
        }
        if let Some(style) = &comments.style {
            lines.extend([String::new(), "Write them in my review style:".to_owned(), String::new(), style.clone()]);
        }
    }
    lines.join("\n")
}

/// The title of a review of local changes: what it reads when the reader chose, else the
/// checkout's local changes.
pub fn local_title(mode: ReviewMode, branch: Option<&str>, scope: Option<&LocalScope>) -> String {
    let prefix = mode.title_prefix();
    match scope {
        None => format!("{prefix}: local changes{}", branch.map(|branch| format!(" on {branch}")).unwrap_or_default()),
        Some(LocalScope::Uncommitted) => format!("{prefix}: uncommitted changes"),
        Some(LocalScope::Branch { base }) => format!("{prefix}: branch vs {}", base.as_deref().unwrap_or("its base")),
        Some(LocalScope::Commit { sha }) => format!("{prefix}: commit {}", sha.get(..7).unwrap_or(sha)),
    }
}

/// What a review of local changes is told to read, in its first message.
fn local_scope_line(scope: Option<&LocalScope>) -> String {
    match scope {
        None => "Review the uncommitted changes there: `git status`, `git diff HEAD` and the untracked files (`git ls-files --others --exclude-standard`). If there are none, review the branch against the branch it was made from instead (`git diff <base>...HEAD`, with <base> the remote's default branch), and say which of the two you reviewed.".to_owned(),
        Some(LocalScope::Uncommitted) => "Review the uncommitted changes there, and only those: `git status`, `git diff HEAD` and the untracked files (`git ls-files --others --exclude-standard`).".to_owned(),
        Some(LocalScope::Branch { base: None }) => "Review the branch against the branch it was made from: `git diff <base>...HEAD`, with <base> the remote's default branch. Leave out uncommitted changes.".to_owned(),
        Some(LocalScope::Branch { base: Some(base) }) => format!("Review the branch against `{base}`: `git diff {base}...HEAD`. Leave out uncommitted changes."),
        Some(LocalScope::Commit { sha }) => format!("Review commit {sha} and only that commit: `git show {sha}`."),
    }
}

pub struct LocalPrompt<'a> {
    pub mode: ReviewMode,
    pub playbook_path: &'a str,
    pub cwd: &'a str,
    pub branch: Option<&'a str>,
    pub scope: Option<&'a LocalScope>,
    pub focus: Option<&'a str>,
    pub files: &'a PromptFiles,
}

/// The first message of a review of local changes: the depth, where the checkout is, what
/// to diff and where the playbook is.
pub fn local_prompt(input: &LocalPrompt<'_>) -> String {
    let mut lines = vec![
        format!("{} of the local changes in {}{}.", input.mode.label(), input.cwd, input.branch.map(|branch| format!(" on branch {branch}")).unwrap_or_default()),
        String::new(),
        local_scope_line(input.scope),
        String::new(),
        format!("Read the review playbook at {} and follow it for these changes. Do not change my working tree, index or branch, and post nothing to the host.", input.playbook_path),
    ];
    file_lines(&mut lines, input.focus, input.files);
    lines.extend([String::new(), format!("When you call {SUBMIT_FINDINGS_TOOL}, pass target \"local\".")]);
    lines.join("\n")
}

/// The first message of the thread that learns a project's review skill.
pub fn learn_skill_prompt(playbook_path: &str) -> String {
    format!("Learn how this project is reviewed, so later reviews of it know what to look for.\n\nRead the playbook at {playbook_path} and follow it. Change nothing in my working tree, post nothing to the host, and hand over what you learn with {SUBMIT_SKILL_TOOL}.")
}

/// The title of the thread that learns a project's review skill.
pub fn learn_skill_title(project_name: &str) -> String {
    format!("Learn: how {project_name} is reviewed")
}
