function respond(response, statusCode, payload) {
  response.writeHead(statusCode, {
    'content-type': 'application/json; charset=utf-8'
  });
  response.end(JSON.stringify(payload));
}

export function healthHandler(_request, response) {
  respond(response, 200, {
    status: 'ok',
    timestamp: new Date().toISOString()
  });
}

export function readinessHandler(_request, response) {
  respond(response, 200, {
    status: 'ready',
    timestamp: new Date().toISOString()
  });
}
