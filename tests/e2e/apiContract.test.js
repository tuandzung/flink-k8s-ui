import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { createApp } from '../../apps/api/src/server.js';
import fixture from '../../apps/api/src/fixtures/jobs.json' with { type: 'json' };

function sortFixtureJobs(jobs) {
  return [...jobs].sort((left, right) => {
    const clusterCompare = left.cluster.localeCompare(right.cluster);
    if (clusterCompare !== 0) {
      return clusterCompare;
    }

    const namespaceCompare = left.namespace.localeCompare(right.namespace);
    if (namespaceCompare !== 0) {
      return namespaceCompare;
    }

    return left.resourceName.localeCompare(right.resourceName);
  });
}

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

test('fixture mode preserves the current HTTP API contract for migration parity', async () => {
  const app = await startFixtureServer();

  try {
    const jobsResponse = await fetch(`${app.baseUrl}/api/jobs`);
    const jobsPayload = await jobsResponse.json();
    assert.equal(jobsResponse.status, 200);
    assert.deepEqual(jobsPayload.jobs, sortFixtureJobs(fixture.jobs));
    assert.equal(jobsPayload.meta.total, fixture.jobs.length);
    assert.match(
      jobsPayload.meta.generatedAt,
      /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/
    );

    const clustersResponse = await fetch(`${app.baseUrl}/api/clusters`);
    const clustersPayload = await clustersResponse.json();
    assert.equal(clustersResponse.status, 200);
    assert.deepEqual(clustersPayload, {
      clusters: [{ name: 'demo' }, { name: 'prod' }]
    });

    const detailResponse = await fetch(
      `${app.baseUrl}/api/jobs/demo/analytics/FlinkDeployment/orders-stream`
    );
    const detailPayload = await detailResponse.json();
    assert.equal(detailResponse.status, 200);
    assert.deepEqual(
      detailPayload,
      {
        job: sortFixtureJobs(fixture.jobs).find(
          (job) =>
            job.cluster === 'demo' &&
            job.namespace === 'analytics' &&
            job.kind === 'FlinkDeployment' &&
            job.resourceName === 'orders-stream'
        )
      }
    );

    const healthzResponse = await fetch(`${app.baseUrl}/healthz`);
    const healthzPayload = await healthzResponse.json();
    assert.equal(healthzResponse.status, 200);
    assert.equal(healthzPayload.status, 'ok');
    assert.match(healthzPayload.timestamp, /^\d{4}-\d{2}-\d{2}T/);

    const readyzResponse = await fetch(`${app.baseUrl}/readyz`);
    const readyzPayload = await readyzResponse.json();
    assert.equal(readyzResponse.status, 200);
    assert.equal(readyzPayload.status, 'ready');
    assert.match(readyzPayload.timestamp, /^\d{4}-\d{2}-\d{2}T/);

    const metricsResponse = await fetch(`${app.baseUrl}/metrics`);
    const metricsPayload = await metricsResponse.json();
    assert.equal(metricsResponse.status, 200);
    assert.equal(metricsPayload.requestsTotal, 6);
    assert.equal(metricsPayload.errorsTotal, 0);
    assert.deepEqual(metricsPayload.routes, {
      listJobs: 1,
      getClusters: 1,
      getJob: 1,
      healthz: 1,
      readyz: 1
    });
    assert.match(metricsPayload.startedAt, /^\d{4}-\d{2}-\d{2}T/);
  } finally {
    await app.close();
  }
});
