import test from 'node:test';
import assert from 'node:assert/strict';
import {
  checkingEntries, fencedCode, findingLocation, groupBySeverity, issueCount, lessonStanding, nextRating, rereviewSummary,
  reviewPhase, scopeLabel, scoreDots, severityHeading, skillSummary, soFarText, withRating,
} from '../src/review-model.ts';

const finding = (severity, title, extra = {}) => ({ severity, confidence: 80, path: 'src/a.ts', title, detail: 'd', ...extra });
const state = (findings, extra = {}) => ({ started: { mode: 'quick' }, findingsActivityId: findings ? 'act' : null, findings, ratings: {}, posted: {}, fixStarted: null, ...extra });
const payload = (extra = {}) => ({ verdict: 'approve', mergeConfidence: 4, reviewEffort: 2, findings: [], needsChecking: [], target: { kind: 'local' }, ...extra });

test('a location cites the path, its line, and a range only when it spans lines', () => {
  assert.equal(findingLocation({ path: 'README.md' }), 'README.md');
  assert.equal(findingLocation({ path: 'a.ts', line: 4 }), 'a.ts:4');
  assert.equal(findingLocation({ path: 'a.ts', line: 4, endLine: 4 }), 'a.ts:4');
  assert.equal(findingLocation({ path: 'a.ts', line: 4, endLine: 9 }), 'a.ts:4-9');
});

test('a suggested fix becomes a fence longer than any backtick run, named by the extension, indentation kept', () => {
  assert.equal(fencedCode('\n\n  let a = 1;\n', 'src/a.ts'), '```ts\n  let a = 1;\n```');
  assert.equal(fencedCode('x = ```y```', 'Makefile'), '````\nx = ```y```\n````');
  assert.equal(fencedCode('a', 'dir.v2/file'), '```\na\n```');
});

test('findings group by severity, most severe first, each keeping the key of its place in the set', () => {
  const groups = groupBySeverity([finding('nit', 'n'), finding('blocker', 'b'), finding('nit', 'n2'), finding('question', 'q')]);
  assert.deepEqual(groups.map(group => group.severity), ['blocker', 'nit', 'question']);
  assert.deepEqual(groups[1].items.map(item => item.key), ['finding:0', 'finding:2']);
  assert.deepEqual(checkingEntries([finding('important', 'c')]).map(item => item.key), ['check:0']);
  assert.equal(severityHeading('blocker', 2), 'Blockers');
  assert.equal(severityHeading('important', 3), 'Important');
  assert.equal(severityHeading('nit', 1), 'Nit');
});

test('a re-review says what was fixed; a first review says nothing', () => {
  assert.equal(rereviewSummary({ previous: { fixed: 2, stillOpen: 1 } }), 'Re-review · 2 fixed · 1 still open');
  assert.equal(rereviewSummary({ incremental: true }), 'Re-review');
  assert.equal(rereviewSummary({}), null);
});

test('the phase follows the records and whether the reviewer still works', () => {
  assert.equal(reviewPhase(null, false), 'none');
  assert.equal(reviewPhase(state(null, { started: null }), true), 'none');
  assert.equal(reviewPhase(state(null), true), 'starting');
  assert.equal(reviewPhase(state(null), false), 'ended');
  assert.equal(reviewPhase(state(payload({ complete: false })), true), 'live');
  assert.equal(reviewPhase(state(payload({ complete: false })), false), 'ended');
  assert.equal(reviewPhase(state(payload({ complete: true })), false), 'done');
  // Findings from before `complete` existed are final.
  assert.equal(reviewPhase(state(payload()), true), 'done');
});

test('counts read as words', () => {
  assert.equal(issueCount(payload()), 'No issues');
  assert.equal(issueCount(payload({ findings: [finding('nit', 'a')], needsChecking: [finding('nit', 'b')] })), '2 issues');
  assert.equal(soFarText(null), 'No findings yet');
  assert.equal(soFarText(payload({ findings: [finding('nit', 'a')] })), '1 finding so far');
});

test('pressing a rating a finding has takes it back; the change shows before the backend answers', () => {
  assert.equal(nextRating(undefined, 'up'), 'up');
  assert.equal(nextRating('up', 'up'), null);
  assert.equal(nextRating('up', 'down'), 'down');
  const rated = withRating(state(payload(), { ratings: { 'finding:0': 'up' } }), 'finding:1', 'down');
  assert.deepEqual(rated.ratings, { 'finding:0': 'up', 'finding:1': 'down' });
  assert.deepEqual(withRating(rated, 'finding:0', null).ratings, { 'finding:1': 'down' });
});

test('a lesson says how it was rated, or that ratings turned it off', () => {
  assert.equal(lessonStanding({ helpful: 0, unhelpful: 0 }), 'not rated yet');
  assert.equal(lessonStanding({ helpful: 2, unhelpful: 1 }), '2 helpful, 1 not');
  assert.equal(lessonStanding({ helpful: 0, unhelpful: 3, disabledByRatings: true }), 'off after your ratings');
});

test('the skill line names its state', () => {
  const skill = { projectId: 'p', status: 'learned', learningThreadId: null, learnedAt: '2026-11-03T10:00:00Z', summary: '', lessons: [{}, {}] };
  assert.match(skillSummary(null), /^Not learned yet/);
  assert.equal(skillSummary({ ...skill, status: 'learning' }), 'Learning from this project’s history now.');
  assert.equal(skillSummary(skill, () => '3 Nov'), 'Learned 3 Nov · 2 lessons');
  assert.match(skillSummary({ ...skill, learnedAt: null, lessons: [{}] }), /^Your own lessons · 1 lesson\./);
});

test('scopes and scores', () => {
  assert.equal(scopeLabel(undefined), 'local changes');
  assert.equal(scopeLabel({ kind: 'branch', base: 'main' }), 'the branch against main');
  assert.equal(scopeLabel({ kind: 'commit', sha: 'abcdef1234' }), 'commit abcdef1');
  assert.deepEqual(scoreDots(2), [true, true, false, false, false]);
  assert.deepEqual(scoreDots(9).filter(Boolean).length, 5);
  assert.deepEqual(scoreDots(-1).filter(Boolean).length, 0);
});
