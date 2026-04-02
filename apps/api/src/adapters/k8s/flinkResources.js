import fs from 'node:fs/promises';
import http from 'node:http';
import https from 'node:https';
import { URL } from 'node:url';
import { normalizeFlinkDeployment, normalizeFlinkSessionJob } from './normalize.js';

function requestJson(urlString, { bearerToken, caCert, insecureSkipTlsVerify, timeoutMs }) {
  const url = new URL(urlString);
  const transport = url.protocol === 'https:' ? https : http;

  return new Promise((resolve, reject) => {
    const request = transport.request(
      url,
      {
        method: 'GET',
        headers: {
          Accept: 'application/json',
          Authorization: `Bearer ${bearerToken}`
        },
        ca: caCert || undefined,
        rejectUnauthorized: !insecureSkipTlsVerify
      },
      (response) => {
        let body = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => {
          body += chunk;
        });
        response.on('end', () => {
          if (response.statusCode >= 400) {
            const error = new Error(
              `Kubernetes API ${response.statusCode} for ${url.pathname}: ${body.slice(0, 200)}`
            );
            error.statusCode = response.statusCode;
            reject(error);
            return;
          }

          try {
            resolve(JSON.parse(body));
          } catch (error) {
            reject(new Error(`Invalid JSON from ${url.pathname}: ${error.message}`));
          }
        });
      }
    );

    request.on('error', reject);
    request.setTimeout(timeoutMs, () => {
      request.destroy(new Error(`Timed out fetching ${url.pathname}`));
    });
    request.end();
  });
}

function buildNamespacePaths(cluster, plural) {
  const namespaces =
    cluster.namespaces.includes('*') || cluster.namespaces.includes('all')
      ? [null]
      : cluster.namespaces;

  return namespaces.map((namespace) => {
    const prefix = `/apis/flink.apache.org/${cluster.flinkApiVersion}`;
    if (!namespace) {
      return `${prefix}/${plural}`;
    }

    return `${prefix}/namespaces/${namespace}/${plural}`;
  });
}

async function listResourcesForPlural(cluster, plural, timeoutMs) {
  const paths = buildNamespacePaths(cluster, plural);
  const responses = await Promise.all(
    paths.map((path) =>
      requestJson(`${cluster.apiUrl}${path}`, {
        bearerToken: cluster.bearerToken,
        caCert: cluster.caCert,
        insecureSkipTlsVerify: cluster.insecureSkipTlsVerify,
        timeoutMs
      })
    )
  );

  return responses.flatMap((response) => response.items || []);
}

export async function listClusterJobs(cluster, { requestTimeoutMs }) {
  const [deployments, sessionJobs] = await Promise.all([
    listResourcesForPlural(cluster, 'flinkdeployments', requestTimeoutMs),
    listResourcesForPlural(cluster, 'flinksessionjobs', requestTimeoutMs).catch(() => [])
  ]);

  return [
    ...deployments.map((resource) => normalizeFlinkDeployment(resource, cluster)),
    ...sessionJobs.map((resource) => normalizeFlinkSessionJob(resource, cluster))
  ];
}

export async function loadFixtureJobs(fixtureFile) {
  const data = await fs.readFile(fixtureFile, 'utf8');
  const parsed = JSON.parse(data);
  return parsed.jobs || [];
}
