// Real protocol clients against the acceptance harness's disposable web replicas.
// Credentials arrive on stdin only; stdout contains aggregate measurements only.
const { ws: WebSocket } = require('../../bootleg-web/node_modules/playwright-core/lib/utilsBundle.js');
const { randomBytes } = require('node:crypto');
const fixture = require('../../bootleg-core/fixtures/v1/register.json');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
function distribution(values) {
  values.sort((a, b) => a - b);
  const at = p => values.length ? values[Math.min(values.length - 1, Math.ceil(values.length * p) - 1)] : null;
  return { count: values.length, p50: at(.5), p95: at(.95), p99: at(.99), max: at(1) };
}
async function main(config) {
  const metrics = { registration_ms: [], heartbeat_ms: [], snapshot_ms: [], sse_initial_ms: [], sse_age_ms: [], sse_gap_ms: [] };
  const errors = {};
  const fail = name => { errors[name] = (errors[name] || 0) + 1; };
  const sockets = [], timers = [], tasks = [], controllers = [];
  let stopping = false;
  const ids = new Set();
  // Canonical TypeID encoding of an RFC 4122 UUIDv4.
  function id() {
    const alphabet = '0123456789abcdefghjkmnpqrstvwxyz';
    const bytes = randomBytes(16);
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let value = BigInt('0x' + bytes.toString('hex')), suffix = '';
    for (let i = 0; i < 26; i++) { suffix = alphabet[Number(value & 31n)] + suffix; value >>= 5n; }
    return 'container_' + suffix;
  }
  async function viewer(index) {
    const controller = new AbortController(); controllers.push(controller);
    const start = performance.now();
    try {
      const response = await fetch(`${config.origins[index % config.origins.length]}/app/projects/${config.org}/${config.project}/instances/events`, {
        headers: { Cookie: config.cookie }, signal: controller.signal,
      });
      if (response.status !== 200) { fail('sse_http'); return; }
      let pending = '', last = null;
      const decoder = new TextDecoder();
      for await (const chunk of response.body) {
        pending += decoder.decode(chunk, { stream: true }).replaceAll('\r', '');
        let end;
        while ((end = pending.indexOf('\n\n')) >= 0) {
          const event = pending.slice(0, end); pending = pending.slice(end + 2);
          if (!event.includes('event:')) continue;
          if (!event.includes('event: snapshot')) { fail('sse_terminal'); return; }
          const data = JSON.parse(event.split('\n').filter(x => x.startsWith('data:')).map(x => x.slice(5)).join('\n'));
          const now = performance.now();
          if (last === null) metrics.sse_initial_ms.push(now - start);
          else metrics.sse_gap_ms.push(now - last);
          last = now;
          metrics.sse_age_ms.push(Date.now() - Date.parse(data.server_time));
          if (data.items.some(x => ids.has(x.container_id) && x.presence !== 'connected')) fail('false_offline');
        }
      }
      if (!stopping) fail('sse_closed');
    } catch (_) { if (!stopping) fail('sse_transport'); }
  }
  async function agent(index) {
    const container = id(); ids.add(container);
    const start = performance.now();
    await new Promise(resolve => {
      let registered = false, pending = null, session;
      const socket = new WebSocket(config.origins[index % config.origins.length].replace('http:', 'ws:') + '/api/v1/agent/connect', {
        headers: { Authorization: `Bearer ${config.projectToken}`, 'Bootleg-Client': 'smuggler', 'Bootleg-Client-Version': '0.1.0', 'Bootleg-Protocol-Version': '1' },
      });
      sockets.push(socket);
      const deadline = setTimeout(() => { fail('registration_timeout'); socket.terminate(); resolve(); }, 30000);
      socket.on('open', () => socket.send(JSON.stringify({ ...fixture, project_id: config.project, container_id: container })));
      socket.on('message', bytes => {
        try {
          const frame = JSON.parse(bytes);
          if (frame.type === 'registered') {
            registered = true; clearTimeout(deadline);
            metrics.registration_ms.push(performance.now() - start);
            session = frame.smuggler_session_id;
            timers.push(setInterval(() => {
              if (pending !== null) { fail('heartbeat_missing_ack'); return; }
              pending = performance.now();
              if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: 'heartbeat', smuggler_session_id: session, application_state: 'running' }));
            }, frame.heartbeat_interval_seconds * 1000));
            resolve();
          } else if (frame.type === 'heartbeat_ack' && frame.smuggler_session_id === session && pending !== null) {
            metrics.heartbeat_ms.push(performance.now() - pending); pending = null;
          } else { fail('agent_protocol'); }
        } catch (_) { fail('agent_decode'); }
      });
      socket.on('unexpected-response', (_, response) => {
        fail(`agent_http_${response.statusCode}`);
        response.resume(); socket.terminate();
      });
      socket.on('error', () => { if (!stopping) fail('agent_transport'); });
      socket.on('close', () => {
        clearTimeout(deadline);
        if (!stopping) fail(registered ? 'agent_closed' : 'registration_rejected');
        resolve();
      });
    });
  }
  try {
    for (let i = 0; i < config.viewers; i++) tasks.push(viewer(i));
    // Separate viewer startup from sustained snapshot contention. Otherwise the
    // initial HTTP/auth burst can exhaust the independent pending-upgrade quota.
    const readyDeadline = performance.now() + 30000;
    while (metrics.sse_initial_ms.length < config.viewers && performance.now() < readyDeadline && !Object.keys(errors).length) await sleep(50);
    if (metrics.sse_initial_ms.length !== config.viewers) throw new Error('viewer startup');
    // Admit at ten attempts/sec while viewers already read the project.
    for (let i = 0; i < config.agents; i++) { tasks.push(agent(i)); await sleep(100); }
    await Promise.all(tasks.slice(config.viewers));
    const until = performance.now() + config.seconds * 1000;
    while (performance.now() < until) {
      for (const origin of config.origins) {
        const start = performance.now();
        try {
          const response = await fetch(`${origin}/api/v1/projects/${config.project}/instances?limit=200`, {
            headers: { Authorization: `Bearer ${config.userToken}`, 'Bootleg-Client': 'cli', 'Bootleg-Client-Version': '0.1.0', 'Bootleg-Protocol-Version': '1' },
            signal: AbortSignal.timeout(10000),
          });
          if (!response.ok) { fail('snapshot_http'); continue; }
          const data = await response.json();
          metrics.snapshot_ms.push(performance.now() - start);
          if (data.next_cursor) fail('snapshot_truncated');
          const live = data.items.filter(x => ids.has(x.container_id));
          if (live.length !== config.agents) fail('snapshot_missing_agents');
          if (live.some(x => x.presence !== 'connected')) fail('false_offline');
        } catch (_) { fail('snapshot_transport'); }
      }
      await sleep(1000);
    }
  } finally {
    stopping = true;
    timers.forEach(clearInterval);
    controllers.forEach(x => x.abort());
    sockets.forEach(x => x.terminate());
    await Promise.allSettled(tasks);
  }
  if (metrics.registration_ms.length !== config.agents) fail('registration_count');
  if (metrics.sse_initial_ms.length !== config.viewers) fail('viewer_count');
  if (!metrics.heartbeat_ms.length) fail('no_heartbeat_samples');
  return { agents: config.agents, viewers: config.viewers, replicas: config.origins.length, seconds: config.seconds,
    errors, metrics: Object.fromEntries(Object.entries(metrics).map(([k, v]) => [k, distribution(v)])) };
}
let input = '';
process.stdin.on('data', chunk => input += chunk);
process.stdin.on('end', () => main(JSON.parse(input)).then(result => console.log(JSON.stringify(result))).catch(() => { console.log(JSON.stringify({ failure: 'driver_failed' })); process.exitCode = 1; }));
