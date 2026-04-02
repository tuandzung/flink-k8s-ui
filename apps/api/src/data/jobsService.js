import { listClusterJobs, loadFixtureJobs } from '../adapters/k8s/flinkResources.js';
import { enrichJobs } from '../adapters/flink/enrichJobs.js';

function sortJobs(jobs) {
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

export function createJobsService(config) {
  let cache = null;
  let cacheExpiresAt = 0;

  async function loadJobs() {
    if (config.fixtureMode) {
      return loadFixtureJobs(config.fixtureFile);
    }

    const jobs = (
      await Promise.all(
        config.clusters.map((cluster) =>
          listClusterJobs(cluster, { requestTimeoutMs: config.requestTimeoutMs })
        )
      )
    ).flat();

    return enrichJobs(jobs, { requestTimeoutMs: config.requestTimeoutMs });
  }

  async function listJobs({ forceRefresh = false } = {}) {
    const now = Date.now();
    if (!forceRefresh && cache && now < cacheExpiresAt) {
      return cache;
    }

    const jobs = sortJobs(await loadJobs());
    cache = jobs;
    cacheExpiresAt = now + config.cacheTtlMs;
    return jobs;
  }

  async function getJobById(id) {
    const jobs = await listJobs();
    return jobs.find((job) => job.id === id) || null;
  }

  async function getJobByLocator({ cluster, namespace, kind, name }) {
    const jobs = await listJobs();
    return (
      jobs.find(
        (job) =>
          job.cluster === cluster &&
          job.namespace === namespace &&
          job.kind === kind &&
          job.resourceName === name
      ) || null
    );
  }

  return {
    listJobs,
    getJobById,
    getJobByLocator
  };
}
