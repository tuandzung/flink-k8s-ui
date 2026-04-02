import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { createConnection, createServer } from 'node:net';
import { spawn } from 'node:child_process';
import { createApp } from '../../apps/api/src/server.js';

async function getFreePort() {
  const server = createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const { port } = server.address();
  await new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
  return port;
}

async function startNodeFixtureServer() {
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

async function startRustFixtureServer() {
  const port = await getFreePort();
  const child = spawn(
    'cargo',
    ['run', '--quiet', '--manifest-path', 'apps/api-rs/Cargo.toml'],
    {
      cwd: process.cwd(),
      env: {
        ...process.env,
        HOST: '127.0.0.1',
        PORT: String(port),
        FIXTURE_MODE: 'true'
      },
      stdio: ['ignore', 'pipe', 'pipe']
    }
  );

  let stderr = '';
  let stdout = '';
  child.stdout.on('data', (chunk) => {
    stdout += chunk.toString();
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk.toString();
  });

  const baseUrl = `http://127.0.0.1:${port}`;
  try {
    await waitForTcpServer(port);
  } catch (error) {
    child.kill('SIGTERM');
    throw new Error(`Rust server failed to start.\nstdout:\n${stdout}\nstderr:\n${stderr}`);
  }

  return {
    baseUrl,
    close: async () => {
      child.kill('SIGTERM');
      await once(child, 'exit').catch(() => {});
    }
  };
}

async function waitForTcpServer(port, { timeoutMs = 30000 } = {}) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    try {
      await new Promise((resolve, reject) => {
        const socket = createConnection({ host: '127.0.0.1', port });
        socket.once('connect', () => {
          socket.end();
          resolve();
        });
        socket.once('error', reject);
      });
      return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for port ${port}`);
}

function normalizePayload(path, payload) {
  const clone = structuredClone(payload);

  if (path === '/api/jobs') {
    clone.meta.generatedAt = '<timestamp>';
  }

  if (path === '/healthz' || path === '/readyz') {
    clone.timestamp = '<timestamp>';
  }

  if (path === '/metrics') {
    clone.startedAt = '<timestamp>';
  }

  return clone;
}

test('Rust fixture-mode backend matches the Node backend API contract', async () => {
  const nodeApp = await startNodeFixtureServer();
  const rustApp = await startRustFixtureServer();
  const paths = [
    '/api/jobs',
    '/api/clusters',
    '/api/jobs/demo/analytics/FlinkDeployment/orders-stream',
    '/healthz',
    '/readyz',
    '/metrics'
  ];

  try {
    for (const path of paths) {
      const [nodeResponse, rustResponse] = await Promise.all([
        fetch(`${nodeApp.baseUrl}${path}`),
        fetch(`${rustApp.baseUrl}${path}`)
      ]);

      assert.equal(rustResponse.status, nodeResponse.status, `status mismatch for ${path}`);

      const [nodePayload, rustPayload] = await Promise.all([nodeResponse.json(), rustResponse.json()]);
      assert.deepEqual(
        normalizePayload(path, rustPayload),
        normalizePayload(path, nodePayload),
        `payload mismatch for ${path}`
      );
    }
  } finally {
    await Promise.all([nodeApp.close(), rustApp.close()]);
  }
});
