import test from 'node:test';
import assert from 'node:assert/strict';
import {
  filterJobs,
  renderAuthError,
  renderAuthLoading,
  renderDrawer,
  renderSessionChrome,
  renderSignedOutShell,
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
  assert.match(html, /&quot;statusSummary&quot;/);
  assert.doesNotMatch(html, /"metadata"/);
  assert.doesNotMatch(html, /"spec"/);
});

test('renderWarnings highlights partial enrichment state', () => {
  const html = renderWarnings(fixture.jobs);
  assert.match(html, /partial enrichment warnings/);
});

test('renderSignedOutShell renders sign-in call to action', () => {
  const html = renderSignedOutShell({
    loginUrl: '/auth/login',
    title: 'Sign in required',
    message: 'Protected dashboard'
  });

  assert.match(html, /Sign in required/);
  assert.match(html, /Protected dashboard/);
  assert.match(html, /href="\/auth\/login"/);
  assert.match(html, />Sign in</);
});

test('renderAuthLoading and renderAuthError expose bootstrap states', () => {
  assert.match(renderAuthLoading(), /Checking session/);

  const errorHtml = renderAuthError({
    error: 'OIDC discovery failed',
    loginUrl: '/auth/login'
  });
  assert.match(errorHtml, /OIDC discovery failed/);
  assert.match(errorHtml, /Try signing in again/);
});

test('renderSessionChrome shows signed-in identity and sign-out action', () => {
  const html = renderSessionChrome({
    status: 'authenticated',
    authenticated: true,
    user: { name: 'Ada Lovelace', email: 'ada@example.com' },
    logoutUrl: '/auth/logout'
  });

  assert.match(html, /Ada Lovelace/);
  assert.match(html, /ada@example.com/);
  assert.match(html, /form method="post" action="\/auth\/logout"/);
  assert.match(html, />Sign out</);
});
