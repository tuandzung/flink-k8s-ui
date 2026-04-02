import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { createApp } from '../../apps/api/src/server.js';

async function startFixtureServer() {
  const { server } = createApp({
    host: '127.0.0.1',
    port: 0,
    rootDir: process.cwd(),
    fixtureMode: true,
    fixtureFile: 'apps/api/src/fixtures/jobs.json',
    cacheTtlMs: 0,
    requestTimeoutMs: 1000,
    clusters: []
  });

  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();

  return {
    baseUrl: `http://${address.address}:${address.port}`,
    close: () =>
      new Promise((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      })
  };
}

test('fixture mode serves web ui and jobs api', async () => {
  const app = await startFixtureServer();

  try {
    const pageResponse = await fetch(`${app.baseUrl}/`);
    const pageHtml = await pageResponse.text();
    assert.equal(pageResponse.status, 200);
    assert.match(pageHtml, /Jobs \+ Status Dashboard/);

    const jobsResponse = await fetch(`${app.baseUrl}/api/jobs`);
    const jobsPayload = await jobsResponse.json();
    assert.equal(jobsResponse.status, 200);
    assert.equal(jobsPayload.meta.total, 4);

    const detailResponse = await fetch(
      `${app.baseUrl}/api/jobs/demo/analytics/FlinkDeployment/orders-stream`
    );
    const detailPayload = await detailResponse.json();
    assert.equal(detailResponse.status, 200);
    assert.equal(detailPayload.job.resourceName, 'orders-stream');

    const metricsResponse = await fetch(`${app.baseUrl}/metrics`);
    const metricsPayload = await metricsResponse.json();
    assert.equal(metricsResponse.status, 200);
    assert.ok(metricsPayload.requestsTotal >= 3);
  } finally {
    await app.close();
  }
});
