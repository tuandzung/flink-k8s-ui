function json(response, statusCode, payload) {
  response.writeHead(statusCode, {
    'content-type': 'application/json; charset=utf-8'
  });
  response.end(JSON.stringify(payload));
}

export function createJobsRouter(jobsService) {
  function respondWithError(response, fallbackMessage, error) {
    const statusCode =
      Number.isInteger(error?.statusCode) && error.statusCode >= 400 && error.statusCode < 600
        ? error.statusCode
        : 500;

    json(response, statusCode, {
      error: fallbackMessage,
      details: error.message
    });
  }

  return {
    async listJobs(_request, response) {
      try {
        const jobs = await jobsService.listJobs();
        json(response, 200, {
          jobs,
          meta: {
            total: jobs.length,
            generatedAt: new Date().toISOString()
          }
        });
      } catch (error) {
        respondWithError(response, 'Failed to list jobs', error);
      }
    },

    async getClusters(_request, response) {
      try {
        const jobs = await jobsService.listJobs();
        const clusters = [...new Set(jobs.map((job) => job.cluster))].map((name) => ({ name }));
        json(response, 200, { clusters });
      } catch (error) {
        respondWithError(response, 'Failed to list clusters', error);
      }
    },

    async getJob(request, response) {
      try {
        const job = request.params.id
          ? await jobsService.getJobById(decodeURIComponent(request.params.id))
          : await jobsService.getJobByLocator({
              cluster: decodeURIComponent(request.params.cluster),
              namespace: decodeURIComponent(request.params.namespace),
              kind: decodeURIComponent(request.params.kind),
              name: decodeURIComponent(request.params.name)
            });

        if (!job) {
          json(response, 404, {
            error: 'Job not found'
          });
          return;
        }

        json(response, 200, { job });
      } catch (error) {
        respondWithError(response, 'Failed to fetch job', error);
      }
    }
  };
}
