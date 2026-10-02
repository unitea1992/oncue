import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
  anchorMatchesWeekday,
  createBiweeklySchedule,
  createWeeklySchedule,
  dailyToTimeInput,
  formatScheduleJa,
  formatWeeklyScheduleJa,
  isDailyTime,
  isExpiredByMonitorEnd,
  isExpiredOnce,
  nextWeeklyOccurrenceUtc,
  normalizeDailyTime,
  onceFromLocalInput,
  onceToLocalInput,
  parseAnchorDate,
} from '../../node_modules/.schedule-test/schedule.js';

describe('time input', () => {
  it('parses and normalizes time inputs (table)', () => {
    const valid = [
      ['21:00', true],
      ['07:30:15', true],
      ['24:00', false],
      ['9:00', false],
      ['', false],
    ];
    for (const [input, expected] of valid) {
      assert.equal(isDailyTime(input), expected, `isDailyTime(${input})`);
    }
    assert.equal(normalizeDailyTime('07:30:15'), '07:30');
    assert.equal(normalizeDailyTime('21:00'), '21:00');
    assert.equal(normalizeDailyTime('nope'), null);
    assert.equal(dailyToTimeInput('bad'), '');
    assert.equal(dailyToTimeInput('21:00'), '21:00');
  });
});

describe('once conversion', () => {
  it('round-trips JST local input and UTC RFC3339', () => {
    // 2026-12-24 19:00 JST == 10:00 UTC
    assert.equal(onceFromLocalInput('2026-12-24T19:00'), '2026-12-24T10:00:00.000Z');
    assert.equal(onceToLocalInput('2026-12-24T10:00:00.000Z'), '2026-12-24T19:00');
  });

  it('rejects nonexistent dates instead of rolling over', () => {
    assert.equal(onceFromLocalInput('2026-02-30T19:00'), null);
    assert.equal(onceFromLocalInput('not-a-date'), null);
    assert.equal(onceFromLocalInput(''), null);
  });
});

describe('labels and expiry', () => {
  it('formats once schedules', () => {
    assert.match(formatScheduleJa({ kind: 'once', startsAt: '2026-12-24T10:00:00.000Z' }), /19:00/);
  });
  it('allows start until monitor end (T+2min), rejects at/after it', () => {
    const start = '2026-09-06T11:00:00.000Z';
    const now = new Date(start);
    assert.equal(isExpiredOnce(start, new Date(now.getTime() + 60 * 1000)), false);
    assert.equal(isExpiredOnce(start, new Date(now.getTime() + 119 * 1000)), false);
    assert.equal(isExpiredOnce(start, new Date(now.getTime() + 120 * 1000)), true);
    assert.equal(isExpiredOnce(start, new Date(now.getTime() + 180 * 1000)), true);
  });

  it('prefers resolved monitor end when provided', () => {
    const end = '2026-09-06T11:02:00.000Z';
    assert.equal(isExpiredByMonitorEnd(end, new Date('2026-09-06T11:01:00.000Z')), false);
    assert.equal(isExpiredByMonitorEnd(end, new Date('2026-09-06T11:02:00.000Z')), true);
    assert.equal(isExpiredByMonitorEnd('not-a-date'), false);
  });
});

describe('weekly occurrence (interval 1)', () => {
  // 2026-09-06T11:00Z == 日曜 20:00 JST。Rust正本との代表値整合を維持する。
  const now = new Date('2026-09-06T11:00:00Z');
  it('resolves weekly cases (table)', () => {
    const cases = [
      ['same-day future', 0, '21:00', now, '2026-09-06T12:00:00.000Z'],
      ['later weekday', 5, '22:00', now, '2026-09-11T13:00:00.000Z'],
      ['past beyond grace', 0, '19:00', now, '2026-09-13T10:00:00.000Z'],
      ['JST midnight', 1, '00:30', new Date('2026-09-06T14:59:00Z'), '2026-09-06T15:30:00.000Z'],
    ];
    for (const [name, weekday, time, at, expected] of cases) {
      assert.equal(nextWeeklyOccurrenceUtc(weekday, time, 1, null, at)?.toISOString(), expected, name);
    }
    assert.equal(nextWeeklyOccurrenceUtc(7, '22:00', 1, null, now), null);
    assert.equal(nextWeeklyOccurrenceUtc(5, 'bad', 1, null, now), null);
  });
});

describe('biweekly occurrence (interval 2 + anchor)', () => {
  const now = new Date('2026-09-06T11:00:00Z');
  it('resolves biweekly cases (table)', () => {
    assert.equal(nextWeeklyOccurrenceUtc(5, '22:00', 2, '2026-08-28', now)?.toISOString(), '2026-09-11T13:00:00.000Z');
    assert.equal(
      nextWeeklyOccurrenceUtc(5, '22:00', 2, '2026-08-28', new Date('2026-09-11T12:59:00Z'))?.toISOString(),
      '2026-09-11T13:00:00.000Z',
    );
    assert.equal(
      nextWeeklyOccurrenceUtc(5, '22:00', 2, '2026-08-28', new Date('2026-09-11T13:03:00Z'))?.toISOString(),
      '2026-09-25T13:00:00.000Z',
    );
    assert.equal(nextWeeklyOccurrenceUtc(5, '22:00', 2, '2026-08-29', now), null);
    assert.equal(nextWeeklyOccurrenceUtc(5, '22:00', 2, null, now), null);
    assert.equal(nextWeeklyOccurrenceUtc(5, '22:00', 2, 'not-a-date', now), null);
  });
});

describe('weekly/biweekly construction', () => {
  it('creates weekly only from valid input', () => {
    assert.deepEqual(createWeeklySchedule(5, '22:00'), { kind: 'weekly', weekday: 5, time: '22:00' });
    assert.equal(createWeeklySchedule(7, '22:00'), null);
    assert.equal(createWeeklySchedule(5, 'bad'), null);
  });
  it('requires matching anchor for biweekly', () => {
    assert.deepEqual(createBiweeklySchedule(5, '22:00', '2026-08-28'), {
      kind: 'biweekly',
      weekday: 5,
      time: '22:00',
      anchorDate: '2026-08-28',
    });
    assert.equal(createBiweeklySchedule(5, '22:00', '2026-08-29'), null);
    assert.equal(createBiweeklySchedule(5, '22:00', ''), null);
  });
  it('validates anchor dates without rollover', () => {
    assert.equal(anchorMatchesWeekday('2026-08-28', 5), true);
    assert.equal(anchorMatchesWeekday('2026-08-29', 5), false);
    assert.equal(parseAnchorDate('2026-02-30'), null);
  });
  it('formats weekly and biweekly short labels', () => {
    assert.equal(formatWeeklyScheduleJa({ kind: 'weekly', weekday: 0, time: '21:00' }), '毎週日曜 21時');
    assert.equal(
      formatWeeklyScheduleJa({ kind: 'biweekly', weekday: 5, time: '22:30', anchorDate: '2026-08-28' }),
      '隔週金曜 22時30分',
    );
  });
});
