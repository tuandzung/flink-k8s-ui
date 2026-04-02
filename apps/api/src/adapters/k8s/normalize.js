function firstDefined(...values) {
  return values.find((value) => value !== undefined && value !== null && value !== '');
}

function toIsoOrNull(value) {
  if (!value) {
    return null;
  }

  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

function unique(values) {
  return [...new Set(values.filter(Boolean))];
}

function canonicalizeStatus(rawValues) {
  const raw = rawValues
    .map((value) => String(value || '').trim())
    .filter(Boolean)
    .join(' / ');
  const normalized = raw.toLowerCase();

  if (!normalized) {
    return {
      status: 'unknown',
      health: 'unknown',
      rawStatus: 'Unknown'
    };
  }

  if (
    normalized.includes('error') ||
    normalized.includes('fail') ||
    normalized.includes('missing')
  ) {
    return { status: 'failed', health: 'error', rawStatus: raw };
  }

  if (normalized.includes('suspend')) {
    return { status: 'suspended', health: 'warning', rawStatus: raw };
  }

  if (
    normalized.includes('reconcil') ||
    normalized.includes('deploy') ||
    normalized.includes('progress') ||
    normalized.includes('upgrad')
  ) {
    return { status: 'reconciling', health: 'warning', rawStatus: raw };
  }

  if (
    normalized.includes('run') ||
    normalized.includes('ready') ||
    normalized.includes('stable')
  ) {
    return { status: 'running', health: 'healthy', rawStatus: raw };
  }

  return { status: 'unknown', health: 'unknown', rawStatus: raw };
}

function buildWarnings(resource, extraWarnings = []) {
  const warnings = [...extraWarnings];
  const error =
    resource?.status?.error ||
    resource?.status?.jobStatus?.error ||
    resource?.status?.errorMessage ||
    resource?.status?.reconciliationStatus?.error;

  if (error) {
    warnings.push(error);
  }

  return unique(warnings);
}

function extractLastUpdatedAt(resource) {
  return toIsoOrNull(
    firstDefined(
      resource?.status?.reconciliationStatus?.lastReconciledAt,
      resource?.status?.jobStatus?.updateTime,
      resource?.metadata?.creationTimestamp
    )
  );
}

function extractDeploymentMode(spec) {
  return firstDefined(spec.mode, spec.deploymentMode, spec?.job?.upgradeMode, null);
}

function normalizeBase({
  cluster,
  kind,
  resource,
  jobName,
  rawStatusValues,
  nativeUiUrl,
  warnings = []
}) {
  const metadata = resource?.metadata || {};
  const spec = resource?.spec || {};
  const statusInfo = canonicalizeStatus(rawStatusValues);
  const namespace = metadata.namespace || 'default';
  const resourceName = metadata.name || jobName || 'unknown';

  return {
    id: `${cluster.name}:${namespace}:${kind}:${resourceName}`,
    cluster: cluster.name,
    namespace,
    kind,
    resourceName,
    jobName: jobName || resourceName,
    status: statusInfo.status,
    health: statusInfo.health,
    rawStatus: statusInfo.rawStatus,
    flinkVersion: firstDefined(spec.flinkVersion, statusInfo.flinkVersion, null),
    deploymentMode: extractDeploymentMode(spec),
    lastUpdatedAt: extractLastUpdatedAt(resource),
    startedAt: toIsoOrNull(
      firstDefined(
        resource?.status?.jobStatus?.startTime,
        resource?.status?.jobManagerDeploymentStatus?.startTime,
        metadata.creationTimestamp
      )
    ),
    nativeUiUrl,
    warnings: buildWarnings(resource, warnings),
    details: {
      metadata,
      spec,
      status: resource?.status || {}
    }
  };
}

export function normalizeFlinkDeployment(resource, cluster) {
  const spec = resource?.spec || {};
  const status = resource?.status || {};
  const jobStatus = status.jobStatus || {};

  return normalizeBase({
    cluster,
    kind: 'FlinkDeployment',
    resource,
    jobName: firstDefined(
      spec?.job?.name,
      spec?.job?.entryClass,
      resource?.metadata?.labels?.['app.kubernetes.io/name'],
      resource?.metadata?.name
    ),
    rawStatusValues: [
      jobStatus.state,
      status.lifecycleState,
      status.jobManagerDeploymentStatus,
      status.reconciliationStatus?.state,
      spec?.job?.state
    ],
    nativeUiUrl: firstDefined(status.jobManagerUrl, status.jobManagerInfo?.url, null)
  });
}

export function normalizeFlinkSessionJob(resource, cluster) {
  const spec = resource?.spec || {};
  const status = resource?.status || {};

  return normalizeBase({
    cluster,
    kind: 'FlinkSessionJob',
    resource,
    jobName: firstDefined(spec.job?.name, spec.job?.jarURI, resource?.metadata?.name),
    rawStatusValues: [
      status.jobStatus?.state,
      status.lifecycleState,
      status.reconciliationStatus?.state,
      spec?.job?.state
    ],
    nativeUiUrl: firstDefined(status.jobManagerUrl, null)
  });
}

export function normalizeFlinkResource(resource, cluster) {
  const kind = resource?.kind;

  if (kind === 'FlinkSessionJob') {
    return normalizeFlinkSessionJob(resource, cluster);
  }

  return normalizeFlinkDeployment(resource, cluster);
}
