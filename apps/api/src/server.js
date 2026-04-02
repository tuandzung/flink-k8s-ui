import fs from 'node:fs/promises';
import path from 'node:path';
import { createServer as createHttpServer } from 'node:http';
import { fileURLToPath } from 'node:url';
import { getAppConfig } from './config.js';
import { createJobsService } from './data/jobsService.js';
import { createJobsRouter } from './routes/jobs.js';
import { healthHandler, readinessHandler } from './routes/health.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8'
};

function resolveStaticFile(config, pathname) {
  const safePath = pathname === '/' ? '/index.html' : pathname;
  const publicRoot = path.resolve(config.rootDir, 'apps/web/public');

  if (safePath.startsWith('/src/')) {
    return path.resolve(config.rootDir, `apps/web${safePath}`);
  }

  return path.resolve(publicRoot, `.${safePath}`);
}

async function serveStaticFile(config, requestPath, response) {
  const filePath = resolveStaticFile(config, requestPath);
  const allowedRoots = [
    path.resolve(config.rootDir, 'apps/web/public'),
    path.resolve(config.rootDir, 'apps/web/src')
  ];
  if (!allowedRoots.some((root) => filePath.startsWith(root))) {
    response.writeHead(403);
    response.end('Forbidden');
    return;
  }

  try {
    const content = await fs.readFile(filePath);
    response.writeHead(200, {
      'content-type':
        MIME_TYPES[path.extname(filePath).toLowerCase()] || 'application/octet-stream'
    });
    response.end(content);
  } catch {
    response.writeHead(404);
    response.end('Not found');
  }
}

function matchRoute(method, pathname) {
  if (method === 'GET' && pathname === '/api/jobs') {
    return { name: 'listJobs' };
  }

  if (method === 'GET' && pathname === '/api/clusters') {
    return { name: 'getClusters' };
  }

  const locatorMatch = pathname.match(
    /^\/api\/jobs\/([^/]+)\/([^/]+)\/([^/]+)\/([^/]+)$/
  );
  if (method === 'GET' && locatorMatch) {
    return {
      name: 'getJob',
      params: {
        cluster: locatorMatch[1],
        namespace: locatorMatch[2],
        kind: locatorMatch[3],
        name: locatorMatch[4]
      }
    };
  }

  if (method === 'GET' && pathname.startsWith('/api/jobs/')) {
    return {
      name: 'getJob',
      params: {
        id: pathname.replace('/api/jobs/', '')
      }
    };
  }

  if (method === 'GET' && pathname === '/healthz') {
    return { name: 'healthz' };
  }

  if (method === 'GET' && pathname === '/readyz') {
    return { name: 'readyz' };
  }

  if (method === 'GET' && pathname === '/metrics') {
    return { name: 'metrics' };
  }

  return null;
}

export function createApp(customConfig = getAppConfig()) {
  const config = customConfig;
  const jobsService = createJobsService(config);
  const jobsRouter = createJobsRouter(jobsService);
  const metrics = {
    startedAt: new Date().toISOString(),
    requestsTotal: 0,
    errorsTotal: 0,
    routes: {}
  };

  const server = createHttpServer(async (request, response) => {
    const url = new URL(request.url, `http://${request.headers.host || 'localhost'}`);
    const route = matchRoute(request.method, url.pathname);
    const startedAt = Date.now();
    metrics.requestsTotal += 1;

    response.on('finish', () => {
      const routeName = route?.name || 'static';
      metrics.routes[routeName] = (metrics.routes[routeName] || 0) + 1;
      if (response.statusCode >= 400) {
        metrics.errorsTotal += 1;
      }

      console.log(
        JSON.stringify({
          level: 'info',
          message: 'request_complete',
          method: request.method,
          path: url.pathname,
          route: routeName,
          statusCode: response.statusCode,
          durationMs: Date.now() - startedAt
        })
      );
    });

    if (route?.name === 'listJobs') {
      await jobsRouter.listJobs(request, response);
      return;
    }

    if (route?.name === 'getClusters') {
      await jobsRouter.getClusters(request, response);
      return;
    }

    if (route?.name === 'getJob') {
      request.params = route.params;
      await jobsRouter.getJob(request, response);
      return;
    }

    if (route?.name === 'healthz') {
      healthHandler(request, response);
      return;
    }

    if (route?.name === 'readyz') {
      readinessHandler(request, response);
      return;
    }

    if (route?.name === 'metrics') {
      response.writeHead(200, {
        'content-type': 'application/json; charset=utf-8'
      });
      response.end(JSON.stringify(metrics));
      return;
    }

    await serveStaticFile(config, url.pathname, response);
  });

  return { server, config };
}

export async function startServer(customConfig = getAppConfig()) {
  const { server, config } = createApp(customConfig);

  await new Promise((resolve) => {
    server.listen(config.port, config.host, resolve);
  });

  console.log(
    JSON.stringify({
      level: 'info',
      message: 'server_started',
      host: config.host,
      port: config.port,
      fixtureMode: config.fixtureMode
    })
  );

  return { server, config };
}

if (process.argv[1] === __filename) {
  startServer().catch((error) => {
    console.error(
      JSON.stringify({
        level: 'error',
        message: 'server_start_failed',
        error: error.message
      })
    );
    process.exitCode = 1;
  });
}
