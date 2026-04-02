import http from 'node:http';
import https from 'node:https';
import { URL } from 'node:url';

function requestJson(urlString, timeoutMs) {
  const url = new URL(urlString);
  const transport = url.protocol === 'https:' ? https : http;

  return new Promise((resolve, reject) => {
    const request = transport.request(
      url,
      {
        method: 'GET',
        headers: {
          Accept: 'application/json'
        }
      },
      (response) => {
        let body = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => {
          body += chunk;
        });
        response.on('end', () => {
          if (response.statusCode >= 400) {
            reject(new Error(`Flink REST ${response.statusCode}: ${body.slice(0, 200)}`));
            return;
          }

          try {
            resolve(JSON.parse(body));
          } catch (error) {
            reject(new Error(`Invalid Flink REST payload: ${error.message}`));
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

function withWarning(job, warning) {
  return {
    ...job,
    warnings: [...(job.warnings || []), warning]
  };
}

export async function enrichJobs(jobs, { requestTimeoutMs = 3000 } = {}) {
  return Promise.all(
    jobs.map(async (job) => {
      if (!job.nativeUiUrl) {
        return withWarning(job, 'Flink REST enrichment unavailable: no JobManager URL');
      }

      try {
        const baseUrl = new URL(job.nativeUiUrl);
        const overview = await requestJson(new URL('/jobs/overview', baseUrl), requestTimeoutMs);
        const candidate = (overview.jobs || []).find(
          (item) => item.name === job.jobName || item.state?.toLowerCase() === job.status
        );

        if (!candidate) {
          return withWarning(job, 'Flink REST enrichment returned no matching job');
        }

        return {
          ...job,
          flinkJobId: candidate.jid || null,
          startedAt: candidate['start-time']
            ? new Date(candidate['start-time']).toISOString()
            : job.startedAt,
          rawStatus: candidate.state || job.rawStatus,
          details: {
            ...job.details,
            flinkRestOverview: candidate
          }
        };
      } catch (error) {
        return withWarning(job, `Flink REST enrichment failed: ${error.message}`);
      }
    })
  );
}
