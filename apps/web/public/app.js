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
  session: { ...DEFAULT_SESSION },
  action: {
    status: 'idle',
    jobId: null,
    action: null,
    message: '',
    deleted: false
  }
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

async function loadJobs(options = {}) {
  if (!canLoadJobs()) {
    render();
    return;
  }

  replaceHtml(elements.content, '<div class="loading-state">Loading Flink jobs…</div>');

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
    const deletedSelectedJobId = options.deletedSelectedJobId || null;
    if (!state.selectedJob && state.jobs[0]) {
      state.selectedJob = state.jobs[0];
    } else if (state.selectedJob) {
      const refreshedSelection = state.jobs.find((job) => job.id === state.selectedJob.id);
      if (refreshedSelection) {
        state.selectedJob = refreshedSelection;
      } else if (deletedSelectedJobId && deletedSelectedJobId === state.selectedJob.id) {
        state.selectedJob = null;
      } else {
        state.selectedJob = state.jobs[0] || null;
      }
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
    replaceHtml(
      elements.content,
      `
      <div class="${noAccess ? 'empty-state' : 'error-state'}">
        <strong>${noAccess ? 'No access to Flink resources.' : 'Failed to load jobs.'}</strong>
        <p>${error.message}</p>
      </div>
    `
    );
  }
}

function render() {
  replaceChildren(elements.sessionChrome, renderSessionChromeNode(state.session));
  elements.refreshButton.disabled = state.session.status === 'loading';

  if (!canLoadJobs()) {
    replaceHtml(elements.filters, '');
    replaceHtml(elements.summary, '');
    replaceChildren(elements.drawer, renderSignedOutDrawerNode(state.session));
    replaceChildren(elements.content, renderSessionStateNode(state.session));
    return;
  }

  const filteredJobs = filterJobs(state.jobs, state.filters);
  replaceHtml(elements.filters, renderFilters(state.jobs, state.filters));
  replaceHtml(elements.summary, `${renderSummary(filteredJobs)}${renderWarnings(filteredJobs)}`);
  replaceHtml(elements.content, renderTable(filteredJobs));
  replaceHtml(elements.drawer, renderDrawer(state.selectedJob, state.action));

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
      replaceHtml(elements.drawer, renderDrawer(state.selectedJob, state.action));
    });
  });

  document.querySelectorAll('[data-job-action]').forEach((button) => {
    button.addEventListener('click', () => {
      const job = state.jobs.find((candidate) => candidate.id === button.dataset.jobId);
      if (!job) {
        return;
      }
      submitJobAction(job, button.dataset.jobAction);
    });
  });
}

function canLoadJobs() {
  return state.session.authenticated || state.session.status === 'legacy';
}

async function submitJobAction(job, action) {
  state.action = {
    status: 'pending',
    jobId: job.id,
    action,
    message: '',
    deleted: false
  };
  render();

  try {
    const response = await fetch(jobActionHref(job, action), {
      method: 'POST',
      headers: state.session.csrfToken
        ? { 'x-csrf-token': state.session.csrfToken }
        : {}
    });
    const payload = await readJson(response);

    if (!response.ok) {
      const error = new Error(payload.details || payload.error || 'Action failed.');
      error.status = response.status;
      throw error;
    }

    state.action = {
      status: 'success',
      jobId: job.id,
      action,
      message: payload.message || 'Action completed successfully.',
      deleted: payload.deleted === true
    };

    await loadJobs({
      deletedSelectedJobId: payload.deleted === true ? job.id : null
    });
  } catch (error) {
    state.action = {
      status: 'error',
      jobId: job.id,
      action,
      message: error instanceof Error ? error.message : 'Action failed.',
      deleted: false
    };
    render();
  }
}

function jobActionHref(job, action) {
  return `/api/jobs/${encodeURIComponent(job.cluster)}/${encodeURIComponent(job.namespace)}/${encodeURIComponent(job.kind)}/${encodeURIComponent(job.resourceName)}/actions/${encodeURIComponent(action)}`;
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

function replaceHtml(element, html) {
  replaceChildren(element, htmlFragment(html));
}

function htmlFragment(html) {
  return document.createRange().createContextualFragment(String(html || ''));
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
        : 'Sign in to view protected job data and action controls';

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
    paragraph.textContent = 'Retry authentication to continue to the protected v2 dashboard.';
    return paragraph;
  }

  paragraph.textContent =
    'Sign in to inspect deployment details, warnings, and single-resource v2 job actions.';
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
      'We’re verifying whether you already have an active session before loading Flink job data and v2 action controls.';
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
    title.textContent = session.title || 'Sign in to manage Flink jobs';
    message.textContent =
      session.message ||
      'This dashboard only loads protected cluster status, job details, and action controls after the server confirms an authenticated session.';

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
