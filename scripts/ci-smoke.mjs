import { spawn } from 'node:child_process';
import process from 'node:process';

const rootDir = process.cwd();
const imageTag = process.env.FLINK_UI_SMOKE_IMAGE || 'flink-job-ui:ci-smoke';
const containerName = `flink-job-ui-smoke-${process.pid}`;
const port = Number(process.env.FLINK_UI_SMOKE_PORT || '3410');

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: rootDir,
      stdio: 'inherit',
      env: process.env,
      ...options
    });

    child.on('exit', (code) => {
      if (code === 0) {
        resolve();
        return;
      }
      reject(new Error(`${command} ${args.join(' ')} exited with code ${code}`));
    });

    child.on('error', reject);
  });
}

async function runJsonCheck() {
  const script = `
import json, urllib.request
base='http://127.0.0.1:${port}'
jobs = json.load(urllib.request.urlopen(base + '/api/jobs'))
clusters = json.load(urllib.request.urlopen(base + '/api/clusters'))
health = json.load(urllib.request.urlopen(base + '/healthz'))
assert jobs['meta']['total'] == 4, jobs
assert clusters == {'clusters': [{'name': 'demo'}, {'name': 'prod'}]}, clusters
assert health['status'] == 'ok', health
print('container smoke ok')
`;

  await run('python', ['-c', script]);
}

async function cleanupContainer() {
  try {
    await run('docker', ['rm', '-f', containerName], { stdio: 'ignore' });
  } catch {}
}

async function main() {
  console.log('==> cargo test');
  await run('cargo', ['test'], { cwd: `${rootDir}/apps/api-rs` });

  console.log('==> npm test');
  await run('npm', ['test']);

  console.log('==> docker build');
  await run('docker', ['build', '-f', 'deploy/api/Dockerfile', '-t', imageTag, '.']);

  console.log('==> docker run');
  await cleanupContainer();
  await run('docker', [
    'run',
    '--rm',
    '-d',
    '--name',
    containerName,
    '-e',
    'FIXTURE_MODE=true',
    '-p',
    `${port}:3000`,
    imageTag
  ]);

  try {
    await new Promise((resolve) => setTimeout(resolve, 1000));
    console.log('==> container smoke check');
    await runJsonCheck();
  } finally {
    await cleanupContainer();
  }
}

main().catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
