import test from 'node:test';
import assert from 'node:assert/strict';
import { DRAG_SLOP, nearestStop, stopForKey, stopFraction, thumbLeft, travelFraction } from '../src/effort-track.ts';

test('stops are spread evenly along the travel, and a lone stop sits at the start', () => {
  assert.deepEqual([0, 1, 2, 3, 4].map(index => stopFraction(index, 5)), [0, 0.25, 0.5, 0.75, 1]);
  assert.equal(stopFraction(0, 1), 0);
  assert.equal(stopFraction(7, 3), 1);
  assert.equal(stopFraction(-2, 3), 0);
});

test('a released thumb snaps to the nearest stop, never past either end', () => {
  assert.equal(nearestStop(0, 5), 0);
  assert.equal(nearestStop(0.12, 5), 0);
  assert.equal(nearestStop(0.13, 5), 1);
  assert.equal(nearestStop(0.62, 5), 2);
  assert.equal(nearestStop(0.63, 5), 3);
  assert.equal(nearestStop(1, 5), 4);
  assert.equal(nearestStop(1.8, 5), 4);
  assert.equal(nearestStop(-0.4, 5), 0);
  assert.equal(nearestStop(0.9, 1), 0);
  // Every stop is the nearest to itself.
  for (const count of [2, 3, 4, 6, 7]) for (let index = 0; index < count; index++) assert.equal(nearestStop(stopFraction(index, count), count), index);
});

test('a dragged thumb follows the pointer by where it was grabbed, between the first stop and the last', () => {
  // A 60px thumb grabbed 20px from its left edge, on a track whose stops start at 2 and end at 242.
  assert.equal(thumbLeft(100, 20, 2, 242), 80);
  assert.equal(thumbLeft(5, 20, 2, 242), 2);
  assert.equal(thumbLeft(900, 20, 2, 242), 242);
  // A track with a single stop has nowhere to go.
  assert.equal(thumbLeft(500, 30, 2, 2), 2);
  assert.equal(travelFraction(2, 2, 242), 0);
  assert.equal(travelFraction(122, 2, 242), 0.5);
  assert.equal(travelFraction(242, 2, 242), 1);
  assert.equal(travelFraction(400, 2, 242), 1);
  assert.equal(travelFraction(2, 2, 2), 0);
  // Pointer to stop, end to end: 4 stops 80px apart, thumb held by its middle.
  const stopAt = pointer => nearestStop(travelFraction(thumbLeft(pointer, 30, 2, 242), 2, 242), 4);
  assert.deepEqual([0, 32, 71, 73, 150, 152, 231, 233, 272, 600].map(stopAt), [0, 0, 0, 1, 1, 2, 2, 3, 3, 3]);
});

test('the keys step one stop, jump to an end, and leave other keys alone', () => {
  assert.equal(stopForKey('ArrowRight', 1, 4), 2);
  assert.equal(stopForKey('ArrowDown', 1, 4), 2);
  assert.equal(stopForKey('ArrowLeft', 1, 4), 0);
  assert.equal(stopForKey('ArrowUp', 1, 4), 0);
  assert.equal(stopForKey('ArrowRight', 3, 4), 3);
  assert.equal(stopForKey('ArrowLeft', 0, 4), 0);
  assert.equal(stopForKey('Home', 2, 4), 0);
  assert.equal(stopForKey('End', 2, 4), 3);
  for (const key of ['Enter', ' ', 'a', 'Tab', 'Escape', 'PageUp']) assert.equal(stopForKey(key, 1, 4), null);
});

test('a press only becomes a drag after a few pixels of travel', () => {
  assert.ok(DRAG_SLOP >= 2 && DRAG_SLOP <= 6);
});
