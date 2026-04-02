import {
  filterJobs,
  renderDrawer,
  renderFilters,
  renderSummary,
  renderTable,
  renderWarnings
} from './render.js';

const state = {
  jobs: [],
  filters: {
    cluster: '',
    namespace: '',
    status: '',
    search: ''
  },
  selectedJob: null
};

const elements = {
  filters: document.querySelector('#filters'),
  summary: document.querySelector('#summary'),
  content: document.querySelector('#content'),
  drawer: document.querySelector('#drawer-content'),
  refreshButton: document.querySelector('#refresh-button')
};

async function loadJobs() {
  elements.content.innerHTML = '<div class="loading-state">Loading Flink jobs…</div>';

  try {
    const response = await fetch('/api/jobs');
    const payload = await response.json();

    if (!response.ok) {
      const message = payload.details || payload.error || 'Unknown error';
      throw new Error(`${response.status}: ${message}`);
    }

    state.jobs = payload.jobs;
    if (!state.selectedJob && state.jobs[0]) {
      state.selectedJob = state.jobs[0];
    } else if (state.selectedJob) {
      state.selectedJob =
        state.jobs.find((job) => job.id === state.selectedJob.id) || state.jobs[0] || null;
    }

    render();
  } catch (error) {
    const noAccess = error.message.includes('401') || error.message.includes('403');
    elements.content.innerHTML = `
      <div class="${noAccess ? 'empty-state' : 'error-state'}">
        <strong>${noAccess ? 'No access to Flink resources.' : 'Failed to load jobs.'}</strong>
        <p>${error.message}</p>
      </div>
    `;
  }
}

function render() {
  const filteredJobs = filterJobs(state.jobs, state.filters);
  elements.filters.innerHTML = renderFilters(state.jobs, state.filters);
  elements.summary.innerHTML = `${renderSummary(filteredJobs)}${renderWarnings(filteredJobs)}`;
  elements.content.innerHTML = renderTable(filteredJobs);
  elements.drawer.innerHTML = renderDrawer(state.selectedJob);

  for (const key of ['cluster', 'namespace', 'status', 'search']) {
    const field = document.querySelector(`#${key}`);
    if (!field) {
      continue;
    }

    field.addEventListener('input', (event) => {
      state.filters[key] = event.target.value;
      render();
    });
  }

  document.querySelectorAll('[data-job-id]').forEach((button) => {
    button.addEventListener('click', () => {
      state.selectedJob = state.jobs.find((job) => job.id === button.dataset.jobId) || null;
      elements.drawer.innerHTML = renderDrawer(state.selectedJob);
    });
  });
}

elements.refreshButton.addEventListener('click', loadJobs);

loadJobs();
