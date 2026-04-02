import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';

const SERVICE_ACCOUNT_TOKEN_PATH =
  '/var/run/secrets/kubernetes.io/serviceaccount/token';
const SERVICE_ACCOUNT_CA_PATH =
  '/var/run/secrets/kubernetes.io/serviceaccount/ca.crt';

function readIfExists(filePath) {
  try {
    return fs.readFileSync(filePath, 'utf8').trim();
  } catch {
    return null;
  }
}

function parseBoolean(value, defaultValue = false) {
  if (value == null || value === '') {
    return defaultValue;
  }

  return ['1', 'true', 'yes', 'on'].includes(String(value).toLowerCase());
}

function parseJson(value, fallback) {
  if (!value) {
    return fallback;
  }

  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`Invalid JSON configuration: ${error.message}`);
  }
}

function defaultClusterFromEnv(env) {
  const host = env.KUBERNETES_SERVICE_HOST;
  const port = env.KUBERNETES_SERVICE_PORT || '443';
  const token = env.K8S_BEARER_TOKEN || readIfExists(SERVICE_ACCOUNT_TOKEN_PATH);
  const caCert = env.K8S_CA_CERT || readIfExists(SERVICE_ACCOUNT_CA_PATH);
  const apiUrl =
    env.K8S_API_URL || (host ? `https://${host}:${port}` : null);

  if (!apiUrl || !token) {
    return [];
  }

  return [
    {
      name: env.K8S_CLUSTER_NAME || 'in-cluster',
      apiUrl,
      bearerToken: token,
      caCert,
      insecureSkipTlsVerify: parseBoolean(env.K8S_INSECURE_SKIP_TLS_VERIFY),
      namespaces: (env.WATCH_NAMESPACES || 'default')
        .split(',')
        .map((value) => value.trim())
        .filter(Boolean),
      flinkApiVersion: env.FLINK_K8S_API_VERSION || 'v1beta1',
      flinkRestBaseUrl: env.FLINK_REST_BASE_URL || null
    }
  ];
}

function normalizeClusters(rawClusters, env) {
  if (!Array.isArray(rawClusters)) {
    return [];
  }

  return rawClusters.map((cluster, index) => ({
    name: cluster.name || `cluster-${index + 1}`,
    apiUrl: cluster.apiUrl || cluster.url || null,
    bearerToken:
      cluster.bearerToken ||
      (cluster.bearerTokenFile ? readIfExists(cluster.bearerTokenFile) : null),
    caCert: cluster.caCert || (cluster.caCertFile ? readIfExists(cluster.caCertFile) : null),
    insecureSkipTlsVerify:
      cluster.insecureSkipTlsVerify ??
      parseBoolean(env.K8S_INSECURE_SKIP_TLS_VERIFY),
    namespaces:
      Array.isArray(cluster.namespaces) && cluster.namespaces.length > 0
        ? cluster.namespaces
        : ['default'],
    flinkApiVersion: cluster.flinkApiVersion || env.FLINK_K8S_API_VERSION || 'v1beta1',
    flinkRestBaseUrl: cluster.flinkRestBaseUrl || null
  }));
}

export function getAppConfig(env = process.env) {
  const rootDir = process.cwd();
  const fixtureFile = path.resolve(
    rootDir,
    env.FIXTURE_FILE || 'apps/api/src/fixtures/jobs.json'
  );
  const configuredClusters = normalizeClusters(
    parseJson(env.FLINK_UI_CLUSTERS_JSON, null),
    env
  );
  const clusters =
    configuredClusters.length > 0 ? configuredClusters : defaultClusterFromEnv(env);
  const fixtureMode =
    parseBoolean(env.FIXTURE_MODE, clusters.length === 0) || clusters.length === 0;

  return {
    host: env.HOST || '0.0.0.0',
    port: Number(env.PORT || '3000'),
    rootDir,
    fixtureMode,
    fixtureFile,
    cacheTtlMs: Number(env.CACHE_TTL_MS || '5000'),
    requestTimeoutMs: Number(env.REQUEST_TIMEOUT_MS || '4000'),
    clusters
  };
}
