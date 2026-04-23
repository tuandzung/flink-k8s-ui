import assert from 'node:assert/strict';
import http from 'node:http';
import { once } from 'node:events';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const rootDir = path.resolve(fileURLToPath(new URL('../..', import.meta.url)));
const apiBinaryPath = path.join(rootDir, 'apps/api-rs/target/debug/flink-job-ui-api');
const sessionCookieName = 'flink_job_ui_session';
const authFlowCookieName = 'flink_job_ui_auth_flow';

let buildPromise;

const tests = [
  [
    'live mode rejects unauthenticated api access with JSON 401',
    async () => {
      const oidc = await startOidcServer();
      const kubernetes = await startJsonServer((request, response) => {
        if (
          request.url ===
          '/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments'
        ) {
          return sendJson(response, 200, { items: [] });
        }
        if (
          request.url ===
          '/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs'
        ) {
          return sendJson(response, 200, { items: [] });
        }
        sendJson(response, 404, { error: 'not found' });
      });
      let app;

      try {
        app = await startApp({
          oidcBaseUrl: oidc.baseUrl,
          clusterConfig: {
            name: 'demo',
            apiUrl: kubernetes.baseUrl,
            bearerToken: 'test-token',
            namespaces: ['analytics'],
            flinkApiVersion: 'v1beta1'
          }
        });

        const response = await request(`${app.baseUrl}/api/jobs`);
        const payload = JSON.parse(response.body);

        assert.equal(response.statusCode, 401);
        assert.match(
          response.headers['content-type'] ?? '',
          /^application\/json\b/
        );
        assert.deepEqual(payload, { error: 'Missing or expired session' });
      } finally {
        await disposeAll(app, kubernetes, oidc);
      }
    }
  ],
  [
    'live mode sanitizes upstream errors for authorized api callers',
    async () => {
      const oidc = await startOidcServer();
      const kubernetes = await startJsonServer((request, response) => {
        if (
          request.url ===
          '/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments'
        ) {
          return sendJson(response, 403, {
            error: 'forbidden secret',
            path: '/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments'
          });
        }
        if (
          request.url ===
          '/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs'
        ) {
          return sendJson(response, 200, { items: [] });
        }
        sendJson(response, 404, { error: 'not found' });
      });
      let app;

      try {
        app = await startApp({
          oidcBaseUrl: oidc.baseUrl,
          clusterConfig: {
            name: 'demo',
            apiUrl: kubernetes.baseUrl,
            bearerToken: 'test-token',
            namespaces: ['analytics'],
            flinkApiVersion: 'v1beta1'
          }
        });
        const sessionCookie = await login(app.baseUrl);

        const response = await request(`${app.baseUrl}/api/jobs`, {
          headers: { cookie: sessionCookie }
        });
        const payload = JSON.parse(response.body);

        assert.equal(response.statusCode, 403);
        assert.equal(payload.error, 'Failed to list jobs');
        assert.equal(
          payload.details,
          'The upstream request failed; check server logs for details'
        );
        const body = JSON.stringify(payload);
        assert.doesNotMatch(body, /forbidden secret/);
        assert.doesNotMatch(body, /flink\.apache\.org/);
      } finally {
        await disposeAll(app, kubernetes, oidc);
      }
    }
  ],
  [
    'live mode ignores untrusted JobManager URLs and still enriches from trusted Flink REST',
    async () => {
      const oidc = await startOidcServer();
      let untrustedHits = 0;
      const untrusted = await startJsonServer((_request, response) => {
        untrustedHits += 1;
        sendJson(response, 200, { error: 'unexpected untrusted call' });
      });
      const trusted = await startJsonServer((request, response) => {
        if (request.url === '/jobs/overview') {
          return sendJson(response, 200, {
            jobs: [
              {
                jid: 'job-123',
                name: 'orders-stream',
                state: 'RUNNING',
                'start-time': 1770000000000
              }
            ]
          });
        }
        sendJson(response, 404, { error: 'not found' });
      });
      const kubernetes = await startJsonServer((request, response) => {
        if (
          request.url ===
          '/apis/flink.apache.org/v1beta1/namespaces/analytics/flinkdeployments'
        ) {
          return sendJson(response, 200, {
            items: [
              {
                kind: 'FlinkDeployment',
                metadata: { name: 'orders-stream', namespace: 'analytics' },
                spec: { job: { name: 'orders-stream', state: 'running' } },
                status: {
                  jobStatus: { state: 'RUNNING' },
                  lifecycleState: 'READY',
                  reconciliationStatus: { state: 'READY' },
                  jobManagerUrl: `${untrusted.baseUrl}/orders-stream/`
                }
              }
            ]
          });
        }
        if (
          request.url ===
          '/apis/flink.apache.org/v1beta1/namespaces/analytics/flinksessionjobs'
        ) {
          return sendJson(response, 200, { items: [] });
        }
        sendJson(response, 404, { error: 'not found' });
      });
      let app;

      try {
        app = await startApp({
          oidcBaseUrl: oidc.baseUrl,
          clusterConfig: {
            name: 'demo',
            apiUrl: kubernetes.baseUrl,
            bearerToken: 'test-token',
            namespaces: ['analytics'],
            flinkApiVersion: 'v1beta1',
            flinkRestBaseUrl: trusted.baseUrl
          }
        });
        const sessionCookie = await login(app.baseUrl);

        const response = await request(`${app.baseUrl}/api/jobs`, {
          headers: { cookie: sessionCookie }
        });
        const payload = JSON.parse(response.body);

        assert.equal(response.statusCode, 200);
        assert.equal(payload.meta.total, 1);
        assert.equal(payload.jobs[0].flinkJobId, 'job-123');
        assert.deepEqual(payload.jobs[0].warnings, [
          'Flink REST enrichment ignored a JobManager URL outside the trusted origin'
        ]);
        assert.equal(untrustedHits, 0);
      } finally {
        await disposeAll(app, kubernetes, trusted, untrusted, oidc);
      }
    }
  ],
  [
    'live mode fails closed when non-loopback Kubernetes TLS verification is disabled',
    async () => {
      await ensureApiBinary();
      const port = await getAvailablePort();
      const appBaseUrl = `http://127.0.0.1:${port}`;
      const clusterConfig = {
        name: 'prod',
        apiUrl: 'https://kubernetes.example.com',
        bearerToken: 'test-token',
        namespaces: ['analytics'],
        flinkApiVersion: 'v1beta1',
        insecureSkipTlsVerify: true
      };

      const child = spawn(apiBinaryPath, {
        cwd: rootDir,
        env: {
          ...process.env,
          HOST: '127.0.0.1',
          PORT: String(port),
          FIXTURE_MODE: 'false',
          REQUEST_TIMEOUT_MS: '1000',
          OIDC_REQUEST_TIMEOUT_MS: '1000',
          OIDC_ISSUER_URL: 'https://issuer.example.com',
          OIDC_CLIENT_ID: 'test-client',
          OIDC_CLIENT_SECRET: 'test-secret',
          OIDC_EXTERNAL_BASE_URL: appBaseUrl,
          OIDC_CALLBACK_PATH: '/auth/callback',
          OIDC_SCOPES: 'openid profile email',
          SESSION_COOKIE_SECRET: '0123456789abcdef0123456789abcdef',
          SESSION_SECURE_COOKIE: 'false',
          FLINK_UI_CLUSTERS_JSON: JSON.stringify([clusterConfig])
        },
        stdio: ['ignore', 'pipe', 'pipe']
      });

      const output = await collectExit(child);
      assert.notEqual(output.code, 0);
      assert.match(output.combined, /K8S_INSECURE_SKIP_TLS_VERIFY/);
    }
  ]
];

await runTests();

async function runTests() {
  let failed = 0;

  for (const [name, fn] of tests) {
    try {
      await fn();
      console.log(`ok - ${name}`);
    } catch (error) {
      failed += 1;
      console.error(`not ok - ${name}`);
      console.error(error);
    }
  }

  if (failed > 0) {
    throw new Error(`${failed} e2e test(s) failed`);
  }
}

async function ensureApiBinary() {
  if (!buildPromise) {
    buildPromise = runCommand('cargo', ['build', '--manifest-path', 'apps/api-rs/Cargo.toml']);
  }

  await buildPromise;
}

function runCommand(command, args) {
  return new Promise((resolve, reject) => {
    let stderr = '';
    const child = spawn(command, args, {
      cwd: rootDir,
      env: process.env,
      stdio: ['ignore', 'ignore', 'pipe']
    });

    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('exit', (code) => {
      if (code === 0) {
        resolve();
        return;
      }

      reject(
        new Error(`${command} ${args.join(' ')} failed with code ${code}\n${stderr}`)
      );
    });
  });
}

async function startApp({ oidcBaseUrl, clusterConfig }) {
  await ensureApiBinary();
  const port = await getAvailablePort();
  const appBaseUrl = `http://127.0.0.1:${port}`;
  const child = spawn(apiBinaryPath, {
    cwd: rootDir,
    env: {
      ...process.env,
      HOST: '127.0.0.1',
      PORT: String(port),
      FIXTURE_MODE: 'false',
      REQUEST_TIMEOUT_MS: '1000',
      OIDC_REQUEST_TIMEOUT_MS: '1000',
      OIDC_ISSUER_URL: oidcBaseUrl,
      OIDC_CLIENT_ID: 'test-client',
      OIDC_CLIENT_SECRET: 'test-secret',
      OIDC_EXTERNAL_BASE_URL: appBaseUrl,
      OIDC_CALLBACK_PATH: '/auth/callback',
      OIDC_SCOPES: 'openid profile email',
      SESSION_COOKIE_SECRET: '0123456789abcdef0123456789abcdef',
      SESSION_SECURE_COOKIE: 'false',
      FLINK_UI_CLUSTERS_JSON: JSON.stringify([clusterConfig])
    },
    stdio: ['ignore', 'pipe', 'pipe']
  });

  let stdout = '';
  let stderr = '';
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });

  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (child.exitCode !== null) {
      throw new Error(`app exited before ready\n${stdout}${stderr}`);
    }

    try {
      const response = await request(`${appBaseUrl}/healthz`);
      if (response.statusCode === 200) {
        return {
          baseUrl: appBaseUrl,
          dispose: async () => {
            await stopChild(child);
          }
        };
      }
    } catch {}

    await delay(100);
  }

  await stopChild(child);
  throw new Error(`app did not become ready\n${stdout}${stderr}`);
}

async function login(appBaseUrl) {
  const loginResponse = await request(`${appBaseUrl}/auth/login`);
  assert.equal(loginResponse.statusCode, 303);

  const authFlowCookie = cookieFromSetCookie(
    getSetCookie(loginResponse),
    authFlowCookieName
  );
  assert.ok(authFlowCookie, 'expected auth flow cookie from /auth/login');

  const state = new URL(loginResponse.headers.location, appBaseUrl).searchParams.get('state');
  assert.ok(state, 'expected OIDC state in login redirect');

  const callbackResponse = await request(
    `${appBaseUrl}/auth/callback?code=test-code&state=${encodeURIComponent(state)}`,
    {
      headers: { cookie: authFlowCookie }
    }
  );
  assert.equal(callbackResponse.statusCode, 303);

  const sessionCookie = cookieFromSetCookie(
    getSetCookie(callbackResponse),
    sessionCookieName
  );
  assert.ok(sessionCookie, 'expected session cookie from /auth/callback');
  return sessionCookie;
}

async function startOidcServer() {
  return startJsonServer((request, response, baseUrl) => {
    if (request.url === '/.well-known/openid-configuration') {
      return sendJson(response, 200, {
        authorization_endpoint: `${baseUrl}/authorize`,
        token_endpoint: `${baseUrl}/token`,
        userinfo_endpoint: `${baseUrl}/userinfo`
      });
    }

    if (request.method === 'POST' && request.url === '/token') {
      return sendJson(response, 200, {
        access_token: 'test-access-token'
      });
    }

    if (request.method === 'GET' && request.url === '/userinfo') {
      return sendJson(response, 200, {
        sub: 'user-123',
        email: 'ada@example.com',
        name: 'Ada Lovelace'
      });
    }

    sendJson(response, 404, { error: 'not found' });
  });
}

async function startJsonServer(handler) {
  const server = http.createServer((request, response) => {
    handler(request, response, baseUrl);
  });
  await listen(server);
  const address = server.address();
  const baseUrl = `http://127.0.0.1:${address.port}`;

  return {
    baseUrl,
    dispose: async () => {
      await closeServer(server);
    }
  };
}

function sendJson(response, statusCode, body) {
  response.writeHead(statusCode, { 'content-type': 'application/json' });
  response.end(JSON.stringify(body));
}

function getSetCookie(response) {
  const value = response.headers['set-cookie'];
  if (Array.isArray(value)) {
    return value;
  }

  return value ? [value] : [];
}

function cookieFromSetCookie(values, cookieName) {
  const match = values.find((value) => value.startsWith(`${cookieName}=`));
  return match ? match.split(';', 1)[0] : null;
}

async function getAvailablePort() {
  const server = http.createServer();
  await listen(server);
  const address = server.address();
  await closeServer(server);
  return address.port;
}

async function stopChild(child) {
  if (child.exitCode !== null) {
    return;
  }

  child.kill('SIGTERM');
  await waitForExitCode(child, 2_000);

  if (child.exitCode === null) {
    child.kill('SIGKILL');
    await waitForExitCode(child, 2_000);
  }
}

async function waitForExitCode(child, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (child.exitCode === null && Date.now() < deadline) {
    await delay(50);
  }
}

async function collectExit(child) {
  let stdout = '';
  let stderr = '';

  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });

  const [code] = await once(child, 'exit');
  return {
    code,
    stdout,
    stderr,
    combined: `${stdout}${stderr}`
  };
}

async function disposeAll(...resources) {
  const errors = [];

  for (const resource of resources.filter(Boolean).reverse()) {
    if (typeof resource.dispose !== 'function') {
      continue;
    }

    try {
      await resource.dispose();
    } catch (error) {
      errors.push(error);
    }
  }

  if (errors.length > 0) {
    throw new AggregateError(errors, 'failed to dispose test resources');
  }
}

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject);
      resolve();
    });
  });
}

function closeServer(server) {
  return new Promise((resolve, reject) => {
    server.close((error) => {
      if (error) {
        reject(error);
        return;
      }

      resolve();
    });
  });
}

function request(url, { method = 'GET', headers = {}, body } = {}) {
  return new Promise((resolve, reject) => {
    const requestInstance = http.request(
      url,
      {
        method,
        headers
      },
      (response) => {
        let responseBody = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => {
          responseBody += chunk;
        });
        response.on('end', () => {
          resolve({
            statusCode: response.statusCode ?? 0,
            headers: response.headers,
            body: responseBody
          });
        });
      }
    );

    requestInstance.on('error', reject);
    requestInstance.setTimeout(2_000, () => {
      requestInstance.destroy(new Error(`request timed out: ${url}`));
    });

    if (body) {
      requestInstance.write(body);
    }
    requestInstance.end();
  });
}
