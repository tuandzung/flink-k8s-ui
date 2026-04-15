import test from 'node:test';
import assert from 'node:assert/strict';
import {
  filterJobs,
  jobManagerProxyHref,
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
  assert.match(
    html,
    /href="\/api\/jobs\/prod%3Arisk%3AFlinkDeployment%3Arisk-detector\/jobmanager-proxy\/"/
  );
  assert.match(html, /Open JobManager UI/);
  assert.match(html, /Status details/);
  assert.match(html, /&quot;statusSummary&quot;/);
  assert.match(html, /data-job-action="cancel"/);
  assert.match(html, /data-job-action="suspend"/);
  assert.match(html, /data-job-action="resume"/);
  assert.doesNotMatch(html, /"metadata"/);
  assert.doesNotMatch(html, /"spec"/);
});

test('renderDrawer reflects enabled and disabled action controls', () => {
  const runningHtml = renderDrawer(fixture.jobs[0]);
  assert.doesNotMatch(runningHtml, /data-job-action="suspend"[^>]*disabled/);
  assert.match(runningHtml, /data-job-action="resume"[^>]*disabled/);

  const suspendedHtml = renderDrawer(fixture.jobs[2]);
  assert.match(suspendedHtml, /data-job-action="suspend"[^>]*disabled/);
  assert.doesNotMatch(suspendedHtml, /data-job-action="resume"[^>]*disabled/);
});

test('renderDrawer shows action feedback and deleted-state success banner', () => {
  const successHtml = renderDrawer(fixture.jobs[0], {
    status: 'success',
    jobId: fixture.jobs[0].id,
    action: 'suspend',
    message: 'Resource suspended successfully.',
    deleted: false
  });
  assert.match(successHtml, /Resource suspended successfully/);

  const deletedHtml = renderDrawer(null, {
    status: 'success',
    jobId: fixture.jobs[0].id,
    action: 'cancel',
    message: 'Resource deleted successfully.',
    deleted: true
  });
  assert.match(deletedHtml, /Resource deleted successfully/);
});

test('renderDrawer escapes untrusted job fields in the action UI path', () => {
  const html = renderDrawer({
    ...fixture.jobs[0],
    kind: '<img src=x onerror=alert(1)>',
    jobName: '<script>alert(1)</script>',
    resourceName: '<b>orders</b>',
    cluster: 'demo"><script>alert(2)</script>',
    namespace: '<svg/onload=alert(3)>',
    warnings: ['<img src=x onerror=alert(4)>']
  });

  assert.doesNotMatch(html, /<script>alert/);
  assert.match(html, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/);
  assert.match(html, /&lt;img src=x onerror=alert\(4\)&gt;/);
});

test('jobManagerProxyHref returns a same-domain proxy path for usable URLs only', () => {
  assert.equal(
    jobManagerProxyHref(fixture.jobs[0]),
    '/api/jobs/demo%3Aanalytics%3AFlinkDeployment%3Aorders-stream/jobmanager-proxy/'
  );
  assert.equal(
    jobManagerProxyHref({
      id: 'demo:analytics:FlinkDeployment:bad-job',
      nativeUiUrl: 'ftp://flink.example.com/orders-stream/'
    }),
    null
  );
  assert.equal(jobManagerProxyHref({ id: 'demo:analytics:FlinkDeployment:no-url' }), null);
});

test('renderWarnings highlights partial enrichment state', () => {
  const html = renderWarnings(fixture.jobs);
  assert.match(html, /partial enrichment warnings/);
});

test('renderSignedOutShell renders v2 sign-in call to action', () => {
  const defaultHtml = renderSignedOutShell();
  assert.match(defaultHtml, /Sign in to manage Flink jobs/);
  assert.match(defaultHtml, /protected cluster status, job details, and action controls/);

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
  const loadingHtml = renderAuthLoading();
  assert.match(loadingHtml, /Checking session/);
  assert.match(loadingHtml, /v2 action controls/);

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
