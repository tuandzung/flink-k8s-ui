import {
  filterJobs,
  renderAuthError,
  renderAuthLoading,
  renderDrawer,
  renderFilters,
  renderSessionChrome,
  renderSignedOutShell,
  renderSummary,
  renderTable,
  renderWarnings
} from './render.js';

const DEFAULT_SESSION = {
  status: 'loading',
  authenticated: false,
  user: null,
  error: null,
  loginUrl: '/auth/login',
  logoutUrl: '/auth/logout'
};

const state = {
  jobs: [],
  filters: {
    cluster: '',
    namespace: '',
    status: '',
    search: ''
  },
  selectedJob: null,
  session: { ...DEFAULT_SESSION }
};

const elements = {
  filters: document.querySelector('#filters'),
  summary: document.querySelector('#summary'),
  content: document.querySelector('#content'),
  drawer: document.querySelector('#drawer-content'),
  refreshButton: document.querySelector('#refresh-button'),
  sessionChrome: document.querySelector('#session-chrome')
};

async function bootstrapSession() {
  state.session = { ...DEFAULT_SESSION, status: 'loading' };
  render();

  try {
    const response = await fetch('/api/session');
    const payload = await readJson(response);

    if (response.status === 404) {
      state.session = {
        ...DEFAULT_SESSION,
        ...normalizeSessionPayload(payload),
        status: 'legacy',
        authenticated: true,
        error: null
      };
      await loadJobs();
      return;
    }

    if (response.status === 401) {
      state.session = {
        ...DEFAULT_SESSION,
        ...normalizeSessionPayload(payload),
        status: 'signed-out',
        authenticated: false,
        error: null
      };
      render();
      return;
    }

    if (!response.ok) {
      throw new Error(payload.error || payload.details || 'Unable to load session state.');
    }

    const session = normalizeSessionPayload(payload);
    if (session.authenticated) {
      state.session = {
        ...DEFAULT_SESSION,
        ...session,
        status: 'authenticated',
        error: null
      };
      await loadJobs();
      return;
    }

    state.session = {
      ...DEFAULT_SESSION,
      ...session,
      status: 'signed-out',
      authenticated: false,
      error: null
    };
    render();
  } catch (error) {
    state.session = {
      ...DEFAULT_SESSION,
      status: 'error',
      error: error instanceof Error ? error.message : 'Unable to determine authentication status.'
    };
    render();
  }
}

async function loadJobs() {
  if (!canLoadJobs()) {
    render();
    return;
  }

  elements.content.innerHTML = '<div class="loading-state">Loading Flink jobs…</div>';

  try {
    const response = await fetch('/api/jobs');
    const payload = await readJson(response);

    if (!response.ok) {
      const message = payload.details || payload.error || 'Unknown error';
      const error = new Error(message);
      error.status = response.status;
      throw error;
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
    const status = error && typeof error === 'object' ? error.status : undefined;
    if (status === 401) {
      if (state.session.status !== 'legacy') {
        state.session = {
          ...state.session,
          authenticated: false,
          status: 'signed-out',
          error: null
        };
        state.jobs = [];
        state.selectedJob = null;
        render();
        return;
      }
    }

    const noAccess = status === 401 || status === 403;
    elements.content.innerHTML = `
      <div class="${noAccess ? 'empty-state' : 'error-state'}">
        <strong>${noAccess ? 'No access to Flink resources.' : 'Failed to load jobs.'}</strong>
        <p>${error.message}</p>
      </div>
    `;
  }
}

function render() {
  elements.sessionChrome.innerHTML = renderSessionChrome(state.session);
  elements.refreshButton.disabled = state.session.status === 'loading';

  if (!canLoadJobs()) {
    elements.filters.innerHTML = '';
    elements.summary.innerHTML = '';
    elements.drawer.innerHTML = renderSignedOutDrawer(state.session);
    elements.content.innerHTML = renderSessionState(state.session);
    return;
  }

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

function canLoadJobs() {
  return state.session.authenticated || state.session.status === 'legacy';
}

function renderSessionState(session) {
  if (session.status === 'loading') {
    return renderAuthLoading();
  }

  if (session.status === 'error') {
    return renderAuthError(session);
  }

  return renderSignedOutShell(session);
}

function renderSignedOutDrawer(session) {
  if (session.status === 'loading') {
    return '<p class="muted">Waiting for session bootstrap before loading dashboard details.</p>';
  }

  if (session.status === 'error') {
    return '<p class="muted">Retry authentication to continue to the protected dashboard.</p>';
  }

  return '<p class="muted">Sign in to inspect deployment details, warnings, and cluster-specific job status.</p>';
}

function normalizeSessionPayload(payload) {
  if (!payload || typeof payload !== 'object') {
    return {};
  }

  return {
    authenticated: payload.authenticated === true,
    user: payload.user && typeof payload.user === 'object' ? payload.user : null,
    loginUrl: typeof payload.loginUrl === 'string' ? payload.loginUrl : DEFAULT_SESSION.loginUrl,
    logoutUrl: typeof payload.logoutUrl === 'string' ? payload.logoutUrl : DEFAULT_SESSION.logoutUrl,
    csrfToken: typeof payload.csrfToken === 'string' ? payload.csrfToken : undefined,
    title: typeof payload.title === 'string' ? payload.title : undefined,
    message: typeof payload.message === 'string' ? payload.message : undefined,
    error: typeof payload.error === 'string' ? payload.error : null
  };
}

async function readJson(response) {
  try {
    return await response.json();
  } catch {
    return {};
  }
}

elements.refreshButton.addEventListener('click', bootstrapSession);

bootstrapSession();
