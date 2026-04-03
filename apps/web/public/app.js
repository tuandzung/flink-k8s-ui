import {
  filterJobs,
  renderDrawer,
  renderFilters,
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
  replaceChildren(elements.sessionChrome, renderSessionChromeNode(state.session));
  elements.refreshButton.disabled = state.session.status === 'loading';

  if (!canLoadJobs()) {
    elements.filters.innerHTML = '';
    elements.summary.innerHTML = '';
    replaceChildren(elements.drawer, renderSignedOutDrawerNode(state.session));
    replaceChildren(elements.content, renderSessionStateNode(state.session));
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

function replaceChildren(element, ...nodes) {
  element.replaceChildren(...nodes.filter(Boolean));
}

function renderSessionChromeNode(session) {
  const wrapper = document.createDocumentFragment();
  const chip = document.createElement('div');
  chip.className = 'session-chip';

  const badge = document.createElement('span');
  badge.className = `status-badge ${session.authenticated || session.status === 'legacy' ? 'status-running' : 'status-unknown'}`;
  badge.textContent =
    session.authenticated || session.status === 'legacy'
      ? session.user?.name || session.user?.email || 'Signed in'
      : session.status === 'loading'
        ? 'Checking session'
        : 'Signed out';

  const detail = document.createElement('span');
  detail.className = 'muted';
  detail.textContent =
    session.authenticated || session.status === 'legacy'
      ? session.user?.email || 'Session active'
      : session.status === 'loading'
        ? 'Loading authentication status…'
        : 'Sign in to view protected job data';

  chip.append(badge, detail);
  wrapper.append(chip);

  const actions = document.createElement('div');
  actions.className = 'session-actions';
  const actionNode = renderSessionActionNode(session);
  if (actionNode) {
    actions.append(actionNode);
  }
  wrapper.append(actions);

  return wrapper;
}

function renderSessionActionNode(session) {
  if (session.authenticated || session.status === 'legacy') {
    const form = document.createElement('form');
    form.method = 'post';
    form.action = session.logoutUrl || '/auth/logout';

    if (session.csrfToken) {
      const csrfInput = document.createElement('input');
      csrfInput.type = 'hidden';
      csrfInput.name = 'csrfToken';
      csrfInput.value = session.csrfToken;
      form.append(csrfInput);
    }

    const button = document.createElement('button');
    button.className = 'secondary-button';
    button.type = 'submit';
    button.textContent = 'Sign out';
    form.append(button);
    return form;
  }

  if (session.status === 'loading') {
    return null;
  }

  const link = document.createElement('a');
  link.className = 'primary-button';
  link.href = session.loginUrl || '/auth/login';
  link.textContent = 'Sign in';
  return link;
}

function renderSignedOutDrawerNode(session) {
  const paragraph = document.createElement('p');
  paragraph.className = 'muted';

  if (session.status === 'loading') {
    paragraph.textContent = 'Waiting for session bootstrap before loading dashboard details.';
    return paragraph;
  }

  if (session.status === 'error') {
    paragraph.textContent = 'Retry authentication to continue to the protected dashboard.';
    return paragraph;
  }

  paragraph.textContent =
    'Sign in to inspect deployment details, warnings, and cluster-specific job status.';
  return paragraph;
}

function renderSessionStateNode(session) {
  const wrapper = document.createElement('div');
  wrapper.className =
    session.status === 'error'
      ? 'auth-card auth-card-error'
      : session.status === 'signed-out'
        ? 'auth-card auth-card-prominent'
        : 'auth-card';

  const eyebrow = document.createElement('p');
  eyebrow.className = 'eyebrow';

  const title = document.createElement('h2');
  const message = document.createElement('p');
  message.className = 'muted';
  const actions = document.createElement('div');
  actions.className = 'auth-actions';

  if (session.status === 'loading') {
    eyebrow.textContent = 'Authentication';
    title.textContent = 'Checking session…';
    message.textContent =
      'We’re verifying whether you already have an active session before loading Flink job data.';
  } else if (session.status === 'error') {
    eyebrow.textContent = 'Authentication error';
    title.textContent = 'We could not verify your session';
    message.textContent = session.error || 'Authentication status could not be determined.';

    const retryLink = document.createElement('a');
    retryLink.className = 'primary-button';
    retryLink.href = session.loginUrl || '/auth/login';
    retryLink.textContent = 'Try signing in again';
    actions.append(retryLink);
  } else {
    eyebrow.textContent = 'Authentication required';
    title.textContent = session.title || 'Sign in to view Flink jobs';
    message.textContent =
      session.message ||
      'This dashboard only loads cluster and job details after the server confirms an authenticated session.';

    const signInLink = document.createElement('a');
    signInLink.className = 'primary-button';
    signInLink.href = session.loginUrl || '/auth/login';
    signInLink.textContent = 'Sign in';
    actions.append(signInLink);
  }

  wrapper.append(eyebrow, title, message);
  if (actions.childNodes.length > 0) {
    wrapper.append(actions);
  }

  return wrapper;
}

elements.refreshButton.addEventListener('click', bootstrapSession);

bootstrapSession();
