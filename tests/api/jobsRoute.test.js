import test from 'node:test';
import assert from 'node:assert/strict';
import { createJobsService } from '../../apps/api/src/data/jobsService.js';

const config = {
  fixtureMode: true,
  fixtureFile: 'apps/api/src/fixtures/jobs.json',
  cacheTtlMs: 0,
  requestTimeoutMs: 1000,
  clusters: []
};

test('jobs service returns fixture rows', async () => {
  const service = createJobsService(config);
  const jobs = await service.listJobs({ forceRefresh: true });

  assert.equal(jobs.length, 4);
  assert.equal(jobs[0].cluster, 'demo');
});

test('jobs service can fetch a job by id', async () => {
  const service = createJobsService(config);
  const job = await service.getJobById('prod:risk:FlinkDeployment:risk-detector');

  assert.equal(job.resourceName, 'risk-detector');
  assert.equal(job.status, 'failed');
});

test('jobs service can fetch a job by planned route locator fields', async () => {
  const service = createJobsService(config);
  const job = await service.getJobByLocator({
    cluster: 'demo',
    namespace: 'analytics',
    kind: 'FlinkDeployment',
    name: 'orders-stream'
  });

  assert.equal(job.jobName, 'orders-stream');
});
