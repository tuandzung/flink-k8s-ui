import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { enrichJobs } from '../../apps/api/src/adapters/flink/enrichJobs.js';

async function startOverviewServer({ statusCode = 200, body, contentType = 'application/json' }) {
  const server = createServer((request, response) => {
    if (request.url !== '/jobs/overview') {
      response.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
      response.end('not found');
      return;
    }

    response.writeHead(statusCode, { 'content-type': contentType });
    response.end(typeof body === 'string' ? body : JSON.stringify(body));
  });

  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();

  return {
    baseUrl: `http://${address.address}:${address.port}/`,
    close: () =>
      new Promise((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      })
  };
}

test('enrichJobs adds a warning when no JobManager URL is available', async () => {
  const [job] = await enrichJobs([
    {
      id: 'demo:analytics:FlinkDeployment:orders-stream',
      jobName: 'orders-stream',
      status: 'running',
      warnings: []
    }
  ]);

  assert.deepEqual(job.warnings, ['Flink REST enrichment unavailable: no JobManager URL']);
});

test('enrichJobs merges Flink REST data when a matching job is returned', async () => {
  const overviewServer = await startOverviewServer({
    body: {
      jobs: [
        {
          jid: 'job-123',
          name: 'orders-stream',
          state: 'RUNNING',
          'start-time': Date.parse('2026-04-02T01:23:45.000Z')
        }
      ]
    }
  });

  try {
    const [job] = await enrichJobs([
      {
        id: 'demo:analytics:FlinkDeployment:orders-stream',
        jobName: 'orders-stream',
        status: 'running',
        rawStatus: 'READY',
        startedAt: '2026-04-01T00:00:00.000Z',
        nativeUiUrl: overviewServer.baseUrl,
        warnings: [],
        details: {}
      }
    ]);

    assert.equal(job.flinkJobId, 'job-123');
    assert.equal(job.rawStatus, 'RUNNING');
    assert.equal(job.startedAt, '2026-04-02T01:23:45.000Z');
    assert.deepEqual(job.details.flinkRestOverview, {
      jid: 'job-123',
      name: 'orders-stream',
      state: 'RUNNING',
      'start-time': Date.parse('2026-04-02T01:23:45.000Z')
    });
    assert.deepEqual(job.warnings, []);
  } finally {
    await overviewServer.close();
  }
});

test('enrichJobs records a warning when Flink REST returns no matching job', async () => {
  const overviewServer = await startOverviewServer({
    body: {
      jobs: [{ jid: 'job-999', name: 'another-job', state: 'FAILED' }]
    }
  });

  try {
    const [job] = await enrichJobs([
      {
        id: 'demo:analytics:FlinkDeployment:orders-stream',
        jobName: 'orders-stream',
        status: 'running',
        nativeUiUrl: overviewServer.baseUrl,
        warnings: [],
        details: {}
      }
    ]);

    assert.deepEqual(job.warnings, ['Flink REST enrichment returned no matching job']);
  } finally {
    await overviewServer.close();
  }
});

test('enrichJobs degrades to warnings when Flink REST returns an error', async () => {
  const overviewServer = await startOverviewServer({
    statusCode: 503,
    body: {
      error: 'upstream unavailable'
    }
  });

  try {
    const [job] = await enrichJobs([
      {
        id: 'demo:analytics:FlinkDeployment:orders-stream',
        jobName: 'orders-stream',
        status: 'running',
        nativeUiUrl: overviewServer.baseUrl,
        warnings: [],
        details: {}
      }
    ]);

    assert.match(job.warnings[0], /^Flink REST enrichment failed: Flink REST 503:/);
  } finally {
    await overviewServer.close();
  }
});
