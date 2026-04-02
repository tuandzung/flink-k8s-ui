import test from 'node:test';
import assert from 'node:assert/strict';
import {
  filterJobs,
  renderDrawer,
  renderSummary,
  renderTable,
  renderWarnings
} from '../../apps/web/public/render.js';
import fixture from '../../fixtures/jobs.json' with { type: 'json' };

test('filterJobs applies status and search filters together', () => {
  const filtered = filterJobs(fixture.jobs, {
    cluster: '',
    namespace: '',
    status: 'failed',
    search: 'risk'
  });

  assert.equal(filtered.length, 1);
  assert.equal(filtered[0].resourceName, 'risk-detector');
});

test('renderSummary includes total and running cards', () => {
  const html = renderSummary(fixture.jobs);
  assert.match(html, /Total jobs/);
  assert.match(html, />4</);
  assert.match(html, /Running/);
});

test('renderTable shows empty state when there are no jobs', () => {
  const html = renderTable([]);
  assert.match(html, /No jobs match the current filters/);
});

test('renderDrawer includes warnings and sanitized status details', () => {
  const html = renderDrawer(fixture.jobs[3]);
  assert.match(html, /Restart backoff exceeded on JobManager/);
  assert.match(html, /Status details/);
  assert.match(html, /"statusSummary"/);
  assert.doesNotMatch(html, /"metadata"/);
  assert.doesNotMatch(html, /"spec"/);
});

test('renderWarnings highlights partial enrichment state', () => {
  const html = renderWarnings(fixture.jobs);
  assert.match(html, /partial enrichment warnings/);
});
