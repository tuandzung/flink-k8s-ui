import test from 'node:test';
import assert from 'node:assert/strict';
import {
  normalizeFlinkDeployment,
  normalizeFlinkSessionJob
} from '../../apps/api/src/adapters/k8s/normalize.js';

const cluster = { name: 'demo' };

test('normalizeFlinkDeployment maps running state to healthy status', () => {
  const resource = {
    kind: 'FlinkDeployment',
    metadata: {
      name: 'orders',
      namespace: 'analytics',
      creationTimestamp: '2026-04-01T00:00:00Z',
      labels: {
        'app.kubernetes.io/name': 'orders-stream'
      }
    },
    spec: {
      flinkVersion: '1.19',
      mode: 'native',
      job: {
        name: 'orders-job'
      }
    },
    status: {
      jobStatus: { state: 'RUNNING' },
      reconciliationStatus: {
        state: 'READY',
        lastReconciledAt: '2026-04-02T00:00:00Z'
      },
      jobManagerUrl: 'https://example/jobs/orders'
    }
  };

  const normalized = normalizeFlinkDeployment(resource, cluster);

  assert.equal(normalized.status, 'running');
  assert.equal(normalized.health, 'healthy');
  assert.equal(normalized.cluster, 'demo');
  assert.equal(normalized.namespace, 'analytics');
  assert.equal(normalized.jobName, 'orders-job');
  assert.equal(normalized.lastUpdatedAt, '2026-04-02T00:00:00.000Z');
});

test('normalizeFlinkSessionJob maps suspended state to warning health', () => {
  const resource = {
    kind: 'FlinkSessionJob',
    metadata: {
      name: 'settlement',
      namespace: 'payments',
      creationTimestamp: '2026-04-01T00:00:00Z'
    },
    spec: {
      job: {
        name: 'settlement-job',
        state: 'SUSPENDED'
      }
    },
    status: {
      jobStatus: { state: 'SUSPENDED' }
    }
  };

  const normalized = normalizeFlinkSessionJob(resource, cluster);

  assert.equal(normalized.status, 'suspended');
  assert.equal(normalized.health, 'warning');
  assert.equal(normalized.jobName, 'settlement-job');
});
