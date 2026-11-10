//! A project's review skill: what its own history teaches a reviewer, learned by an
//! agent that reads how its pull requests were reviewed, and shaped by how the user
//! rates findings. It is the user's, not the repository's: one JSON file per project in
//! the app's data directory, so two people on one repository hold different skills.
//!
//! Every later review trusts the file, so only the thread the app asked to learn may
//! hand lessons over (`accept_learned`). Do not loosen that to "any thread of the
//! project": any agent that reads a hostile comment could then poison every review.

use super::findings::{bounded, check_evidence, Activity, Evidence, Rating};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

pub const MAX_LESSONS: usize = 40;
pub const MAX_LESSON_CHARS: usize = 200;
const MAX_SUMMARY_CHARS: usize = 4_000;
/// A learned lesson turned down this often, and never found helpful, is switched off.
pub const UNHELPFUL_LIMIT: u32 = 3;
const SKILL_DIR: &str = "review-skills";

/// Where a lesson came from: fixes the history shows being made after merging, what the
/// repository's reviewers ask for, or the user who wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LessonSource {
    KeptBreaking,
    ReviewersAsk,
    Yours,
}

/// What the learning agent hands over for one lesson.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedLesson {
    pub text: String,
    /// Where it applies, as path globs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<String>,
    pub source: LessonSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Vec<Evidence>>,
}

/// What the learning agent submits with `submit_review_skill`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedSkill {
    pub summary: String,
    pub lessons: Vec<LearnedLesson>,
}

impl LearnedSkill {
    /// Trims and bounds a hand-over. A learned lesson is never the user's own.
    pub fn normalize(&mut self) -> Result<(), String> {
        if self.summary.encode_utf16().count() > MAX_SUMMARY_CHARS {
            return Err(format!("summary is longer than {MAX_SUMMARY_CHARS} characters."));
        }
        if self.lessons.len() > MAX_LESSONS {
            return Err(format!("A review skill holds at most {MAX_LESSONS} lessons."));
        }
        for (index, lesson) in self.lessons.iter_mut().enumerate() {
            let at = |error: String| format!("lessons[{index}]: {error}");
            bounded(&mut lesson.text, MAX_LESSON_CHARS, "text").map_err(at)?;
            if let Some(place) = &mut lesson.r#where {
                bounded(place, MAX_LESSON_CHARS, "where").map_err(at)?;
            }
            if lesson.source == LessonSource::Yours {
                return Err(at("source is kept-breaking or reviewers-ask.".into()));
            }
            if lesson.occurrences == Some(0) {
                return Err(at("occurrences starts at 1.".into()));
            }
            if let Some(evidence) = &mut lesson.evidence {
                check_evidence(evidence, "A lesson").map_err(at)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lesson {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<String>,
    pub source: LessonSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<u32>,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// Off: reviews are not told about it.
    pub enabled: bool,
    /// Set when the user's own ratings switched it off, so the list can say so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_by_ratings: Option<bool>,
    /// Findings that cited it and were rated up, and down.
    #[serde(default)]
    pub helpful: u32,
    #[serde(default)]
    pub unhelpful: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillStatus {
    /// An agent is reading the history; the lessons are those of the last time.
    Learning,
    Learned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub project_id: String,
    pub status: SkillStatus,
    /// The thread that is learning or last learned, to open or stop.
    pub learning_thread_id: Option<String>,
    /// ISO 8601.
    pub learned_at: Option<String>,
    /// How this project is reviewed, in the learning agent's words.
    pub summary: String,
    pub lessons: Vec<Lesson>,
}

impl Skill {
    pub fn empty(project_id: &str) -> Self {
        Self { project_id: project_id.to_owned(), status: SkillStatus::Learned, learning_thread_id: None, learned_at: None, summary: String::new(), lessons: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SkillEdit {
    #[serde(rename_all = "camelCase")]
    SetEnabled { lesson_id: String, enabled: bool },
    Add {
        text: String,
        #[serde(default)]
        r#where: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Remove { lesson_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillError {
    Lesson,
    Full,
    Invalid,
    Read,
    Write,
    NotLearning,
}

impl SkillError {
    pub fn message(self) -> String {
        match self {
            Self::Lesson => "That lesson is not part of this project's review skill.".into(),
            Self::Full => format!("A review skill holds at most {MAX_LESSONS} lessons."),
            Self::Invalid => format!("A lesson is 1 to {MAX_LESSON_CHARS} characters."),
            Self::Read => "Could not read the project's review skill.".into(),
            Self::Write => "Could not save the project's review skill.".into(),
            Self::NotLearning => "This thread was not asked to learn the project's review skill.".into(),
        }
    }
}

/// A lesson's id from its words, so learning the same lesson again keeps the user's choices.
pub fn lesson_id(text: &str, taken: &HashSet<String>) -> String {
    let mut slug = String::new();
    for character in text.to_lowercase().chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(56);
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "lesson" } else { slug };
    if !taken.contains(slug) {
        return slug.to_owned();
    }
    (2u32..).map(|suffix| format!("{slug}-{suffix}")).find(|candidate| !taken.contains(candidate)).expect("an unused suffix exists")
}

/// The skill after a learning agent hands over what it found: its lessons replace the
/// learned ones, the user's own lessons stay, and a lesson learned again keeps whether it
/// was on.
pub fn merge_learned(previous: Option<&Skill>, learned: &LearnedSkill, project_id: &str, learning_thread_id: Option<&str>, learned_at: &str) -> Skill {
    let earlier_lessons = previous.map(|skill| skill.lessons.as_slice()).unwrap_or_default();
    let before: HashMap<&str, &Lesson> = earlier_lessons.iter().map(|lesson| (lesson.id.as_str(), lesson)).collect();
    let mut lessons: Vec<Lesson> = earlier_lessons.iter().filter(|lesson| lesson.source == LessonSource::Yours).cloned().collect();
    let mut taken: HashSet<String> = lessons.iter().map(|lesson| lesson.id.clone()).collect();
    for lesson in &learned.lessons {
        if lessons.len() >= MAX_LESSONS {
            break;
        }
        let id = lesson_id(&lesson.text, &HashSet::new());
        // The same words twice in one hand-over are one lesson.
        if !taken.insert(id.clone()) {
            continue;
        }
        let earlier = before.get(id.as_str()).filter(|earlier| earlier.source != LessonSource::Yours);
        lessons.push(Lesson {
            id,
            text: lesson.text.clone(),
            r#where: lesson.r#where.clone(),
            source: lesson.source,
            occurrences: lesson.occurrences,
            evidence: lesson.evidence.clone().unwrap_or_default(),
            // Learning a lesson again must not switch back on what the user switched off.
            enabled: earlier.is_none_or(|earlier| earlier.enabled),
            disabled_by_ratings: earlier.and_then(|earlier| earlier.disabled_by_ratings.filter(|set| *set)),
            helpful: earlier.map_or(0, |earlier| earlier.helpful),
            unhelpful: earlier.map_or(0, |earlier| earlier.unhelpful),
        });
    }
    Skill {
        project_id: project_id.to_owned(),
        status: SkillStatus::Learned,
        learning_thread_id: learning_thread_id.map(str::to_owned),
        learned_at: Some(learned_at.to_owned()),
        summary: learned.summary.trim().to_owned(),
        lessons,
    }
}

/// The skill after one edit by the user, or why it cannot be made.
pub fn edit(mut skill: Skill, edit: SkillEdit) -> Result<Skill, SkillError> {
    match edit {
        SkillEdit::SetEnabled { lesson_id, enabled } => {
            let lesson = skill.lessons.iter_mut().find(|lesson| lesson.id == lesson_id).ok_or(SkillError::Lesson)?;
            // The user's own choice replaces whatever their ratings did.
            lesson.disabled_by_ratings = None;
            lesson.enabled = enabled;
        }
        SkillEdit::Add { mut text, mut r#where } => {
            if skill.lessons.len() >= MAX_LESSONS {
                return Err(SkillError::Full);
            }
            bounded(&mut text, MAX_LESSON_CHARS, "text").map_err(|_| SkillError::Invalid)?;
            if let Some(place) = &mut r#where {
                bounded(place, MAX_LESSON_CHARS, "where").map_err(|_| SkillError::Invalid)?;
            }
            let taken = skill.lessons.iter().map(|lesson| lesson.id.clone()).collect();
            let lesson = Lesson {
                id: lesson_id(&text, &taken),
                text,
                r#where,
                source: LessonSource::Yours,
                occurrences: None,
                evidence: Vec::new(),
                enabled: true,
                disabled_by_ratings: None,
                helpful: 0,
                unhelpful: 0,
            };
            skill.lessons.insert(0, lesson);
        }
        SkillEdit::Remove { lesson_id } => {
            let before = skill.lessons.len();
            skill.lessons.retain(|lesson| lesson.id != lesson_id);
            if skill.lessons.len() == before {
                return Err(SkillError::Lesson);
            }
        }
    }
    Ok(skill)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RatingCount {
    pub helpful: u32,
    pub unhelpful: u32,
}

/// How findings that cited each lesson are rated now: each finding counts once, as last
/// rated, and not at all when its rating was taken back. Read oldest first.
pub fn rating_counts(activities: &[Activity], project_id: &str) -> HashMap<String, RatingCount> {
    let mut latest: HashMap<String, Option<(String, bool)>> = HashMap::new();
    for payload in activities.iter().filter_map(Activity::rating) {
        if payload.project_id != project_id {
            continue;
        }
        let Some(lesson_id) = payload.finding.lesson_id else { continue };
        latest.insert(format!("{}/{}", payload.findings_activity_id, payload.key), payload.rating.map(|rating| (lesson_id, rating == Rating::Up)));
    }
    let mut counts: HashMap<String, RatingCount> = HashMap::new();
    for (lesson_id, up) in latest.into_values().flatten() {
        let count = counts.entry(lesson_id).or_default();
        if up {
            count.helpful += 1;
        } else {
            count.unhelpful += 1;
        }
    }
    counts
}

/// The skill with its lessons' ratings counted. A learned lesson that reaches the limit of
/// thumbs-down without one thumbs-up is switched off, once: the user can switch it back
/// on and later ratings leave their choice alone.
pub fn apply_ratings(mut skill: Skill, counts: &HashMap<String, RatingCount>) -> Skill {
    for lesson in &mut skill.lessons {
        let RatingCount { helpful, unhelpful } = counts.get(&lesson.id).copied().unwrap_or_default();
        let reached_limit = lesson.unhelpful < UNHELPFUL_LIMIT && unhelpful >= UNHELPFUL_LIMIT && helpful == 0;
        if reached_limit && lesson.enabled && lesson.source != LessonSource::Yours {
            lesson.enabled = false;
            lesson.disabled_by_ratings = Some(true);
        }
        lesson.helpful = helpful;
        lesson.unhelpful = unhelpful;
    }
    skill
}

/// One lesson as a reviewer reads it. Field names are in the playbook.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewerLesson {
    pub id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#where: Option<String>,
    pub source: LessonSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

/// What a reviewer reads: the summary and the lessons that are on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewerSkill {
    pub summary: String,
    pub lessons: Vec<ReviewerLesson>,
}

pub fn for_reviewer(skill: &Skill) -> ReviewerSkill {
    ReviewerSkill {
        summary: skill.summary.clone(),
        lessons: skill
            .lessons
            .iter()
            .filter(|lesson| lesson.enabled)
            .map(|lesson| ReviewerLesson {
                id: lesson.id.clone(),
                text: lesson.text.clone(),
                r#where: lesson.r#where.clone(),
                source: lesson.source,
                occurrences: lesson.occurrences,
                evidence: lesson.evidence.clone(),
            })
            .collect(),
    }
}

/// Whether a project that was just opened should be learned without the user asking:
/// never learned, nothing learning, a repository to read, a model to read it with, and
/// the setting on. Learning spends the user's plan, so the decision stays here.
pub fn should_auto_learn(skill: Option<&Skill>, is_repository: bool, has_model: bool, auto_learn_setting: bool) -> bool {
    auto_learn_setting && is_repository && has_model && skill.is_none_or(|skill| skill.status != SkillStatus::Learning && skill.learned_at.is_none() && skill.learning_thread_id.is_none())
}

/// The skill files of one data directory.
pub struct SkillStore {
    dir: PathBuf,
}

impl SkillStore {
    pub fn new(data_dir: &Path) -> Self {
        Self { dir: data_dir.join(SKILL_DIR) }
    }

    /// A project id is the app's own, but it names a file: keep it to one path segment.
    fn path(&self, project_id: &str) -> Result<PathBuf, SkillError> {
        let safe = !project_id.is_empty() && project_id.len() <= 128 && project_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if !safe {
            return Err(SkillError::Read);
        }
        Ok(self.dir.join(format!("{project_id}.json")))
    }

    pub fn read(&self, project_id: &str) -> Result<Option<Skill>, SkillError> {
        let path = self.path(project_id)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(SkillError::Read),
        };
        let skill: Skill = serde_json::from_str(&text).map_err(|_| SkillError::Read)?;
        // A file moved in from another project must not speak for this one.
        if skill.project_id != project_id {
            return Err(SkillError::Read);
        }
        Ok(Some(skill))
    }

    /// Swapped in by rename, so a review starting at the same moment reads a whole file.
    pub fn write(&self, skill: &Skill) -> Result<(), SkillError> {
        let path = self.path(&skill.project_id).map_err(|_| SkillError::Write)?;
        std::fs::create_dir_all(&self.dir).map_err(|_| SkillError::Write)?;
        let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        let text = serde_json::to_string_pretty(skill).map_err(|_| SkillError::Write)?;
        std::fs::write(&temporary, text).map_err(|_| SkillError::Write)?;
        std::fs::rename(&temporary, &path).map_err(|_| {
            let _ = std::fs::remove_file(&temporary);
            SkillError::Write
        })
    }

    /// Marks the project as being learned by `thread_id`; the lessons stay those of the last time.
    pub fn begin_learning(&self, project_id: &str, thread_id: &str) -> Result<Skill, SkillError> {
        let mut skill = self.read(project_id)?.unwrap_or_else(|| Skill::empty(project_id));
        skill.status = SkillStatus::Learning;
        skill.learning_thread_id = Some(thread_id.to_owned());
        self.write(&skill)?;
        Ok(skill)
    }

    /// Learning stopped or failed: the skill is what it was, and no thread may submit.
    pub fn cancel_learning(&self, project_id: &str, thread_id: &str) -> Result<Option<Skill>, SkillError> {
        let Some(mut skill) = self.read(project_id)? else { return Ok(None) };
        if skill.status == SkillStatus::Learning && skill.learning_thread_id.as_deref() == Some(thread_id) {
            skill.status = SkillStatus::Learned;
            if skill.learned_at.is_none() {
                skill.learning_thread_id = None;
            }
            self.write(&skill)?;
        }
        Ok(Some(skill))
    }

    /// Takes a hand-over, only from the thread that was asked to learn.
    pub fn accept_learned(&self, project_id: &str, thread_id: &str, mut learned: LearnedSkill, learned_at: &str) -> Result<Skill, SkillError> {
        let previous = self.read(project_id)?;
        let asked = previous.as_ref().is_some_and(|skill| skill.status == SkillStatus::Learning && skill.learning_thread_id.as_deref() == Some(thread_id));
        if !asked {
            return Err(SkillError::NotLearning);
        }
        learned.normalize().map_err(|_| SkillError::Invalid)?;
        let skill = merge_learned(previous.as_ref(), &learned, project_id, Some(thread_id), learned_at);
        self.write(&skill)?;
        Ok(skill)
    }

    pub fn edit(&self, project_id: &str, change: SkillEdit) -> Result<Skill, SkillError> {
        let skill = self.read(project_id)?.unwrap_or_else(|| Skill::empty(project_id));
        let skill = edit(skill, change)?;
        self.write(&skill)?;
        Ok(skill)
    }

    /// Counts the project's ratings into its lessons, after one was recorded.
    pub fn apply_ratings(&self, project_id: &str, activities: &[Activity]) -> Result<Option<Skill>, SkillError> {
        let Some(skill) = self.read(project_id)? else { return Ok(None) };
        let rated = apply_ratings(skill.clone(), &rating_counts(activities, project_id));
        if rated != skill {
            self.write(&rated)?;
        }
        Ok(Some(rated))
    }
}
