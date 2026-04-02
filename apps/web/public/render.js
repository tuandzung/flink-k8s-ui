export function statusClass(value) {
  return `status-badge status-${String(value || 'unknown').toLowerCase()}`;
}

export function summarizeJobs(jobs) {
  const counts = jobs.reduce(
    (summary, job) => {
      summary.total += 1;
      summary[job.status] = (summary[job.status] || 0) + 1;
      return summary;
    },
    { total: 0 }
  );

  return [
    ['Total jobs', counts.total || 0],
    ['Running', counts.running || 0],
    ['Reconciling', counts.reconciling || 0],
    ['Suspended', counts.suspended || 0],
    ['Failed', counts.failed || 0]
  ];
}

export function filterJobs(jobs, filters) {
  return jobs.filter((job) => {
    if (filters.cluster && job.cluster !== filters.cluster) {
      return false;
    }

    if (filters.namespace && job.namespace !== filters.namespace) {
      return false;
    }

    if (filters.status && job.status !== filters.status) {
      return false;
    }

    if (filters.search) {
      const haystack = `${job.resourceName} ${job.jobName}`.toLowerCase();
      if (!haystack.includes(filters.search.toLowerCase())) {
        return false;
      }
    }

    return true;
  });
}

export function renderFilters(jobs, filters) {
  const clusters = [...new Set(jobs.map((job) => job.cluster))];
  const namespaces = [...new Set(jobs.map((job) => job.namespace))];
  const statuses = [...new Set(jobs.map((job) => job.status))];

  const select = (label, key, values) => `
    <div class="field">
      <label for="${key}">${label}</label>
      <select id="${key}">
        <option value="">All</option>
        ${values
          .map(
            (value) =>
              `<option value="${value}" ${filters[key] === value ? 'selected' : ''}>${value}</option>`
          )
          .join('')}
      </select>
    </div>
  `;

  return `
    ${select('Cluster', 'cluster', clusters)}
    ${select('Namespace', 'namespace', namespaces)}
    ${select('Status', 'status', statuses)}
    <div class="field">
      <label for="search">Search</label>
      <input id="search" type="search" value="${filters.search || ''}" placeholder="resource or job name" />
    </div>
  `;
}

export function renderSummary(jobs) {
  return summarizeJobs(jobs)
    .map(
      ([label, value]) => `
        <div class="summary-card">
          <span class="muted">${label}</span>
          <strong>${value}</strong>
        </div>
      `
    )
    .join('');
}

export function renderWarnings(jobs) {
  const warningCount = jobs.filter((job) => (job.warnings || []).length > 0).length;
  if (warningCount === 0) {
    return '';
  }

  return `
    <div class="status-badge status-reconciling">
      ${warningCount} job${warningCount === 1 ? '' : 's'} have partial enrichment warnings
    </div>
  `;
}

export function renderTable(jobs) {
  if (jobs.length === 0) {
    return `<div class="empty-state">No jobs match the current filters.</div>`;
  }

  return `
    <table>
      <thead>
        <tr>
          <th>Job</th>
          <th>Cluster / Namespace</th>
          <th>Kind</th>
          <th>Status</th>
          <th>Updated</th>
        </tr>
      </thead>
      <tbody>
        ${jobs
          .map(
            (job) => `
              <tr>
                <td>
                  <button class="row-button" data-job-id="${job.id}">
                    <strong>${job.jobName}</strong><br />
                    <span class="muted">${job.resourceName}</span>
                  </button>
                </td>
                <td>${job.cluster}<br /><span class="muted">${job.namespace}</span></td>
                <td>${job.kind}</td>
                <td><span class="${statusClass(job.status)}">${job.status}</span></td>
                <td>${job.lastUpdatedAt ? new Date(job.lastUpdatedAt).toLocaleString() : '—'}</td>
              </tr>
            `
          )
          .join('')}
      </tbody>
    </table>
  `;
}

export function renderDrawer(job) {
  if (!job) {
    return `
      <p class="muted">Select a job to inspect deployment details and warnings.</p>
    `;
  }

  return `
    <div>
      <p class="eyebrow">${job.kind}</p>
      <h2>${job.jobName}</h2>
      <p><span class="${statusClass(job.status)}">${job.status}</span></p>
      <p><strong>Cluster:</strong> ${job.cluster}</p>
      <p><strong>Namespace:</strong> ${job.namespace}</p>
      <p><strong>Resource:</strong> ${job.resourceName}</p>
      <p><strong>Flink version:</strong> ${job.flinkVersion || 'unknown'}</p>
      <p><strong>Mode:</strong> ${job.deploymentMode || 'unknown'}</p>
      <p><strong>Last update:</strong> ${job.lastUpdatedAt || '—'}</p>
      ${
        job.nativeUiUrl
          ? `<p><a href="${job.nativeUiUrl}" target="_blank" rel="noreferrer">Open native Flink UI</a></p>`
          : '<p class="muted">No native Flink UI URL available.</p>'
      }
      ${
        job.warnings?.length
          ? `<ul class="warning-list">${job.warnings
              .map((warning) => `<li>${warning}</li>`)
              .join('')}</ul>`
          : '<p class="muted">No warnings reported.</p>'
      }
      <h3>Raw status</h3>
      <pre>${escapeHtml(JSON.stringify(job.details, null, 2))}</pre>
    </div>
  `;
}

function escapeHtml(value) {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;');
}
