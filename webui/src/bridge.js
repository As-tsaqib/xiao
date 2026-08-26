const ACTION = '/data/adb/modules/xiao/action.sh';
const MANAGER_GET = 'manager-get-base64';
const MANAGER_POST = 'manager-post-base64';
const GET_RESOURCES = new Set([
  'dashboard', 'telegram', 'providers', 'runtime', 'context', 'sessions',
  'runs', 'attachments', 'memory', 'skills', 'tools', 'security',
  'diagnostics', 'logs', 'agent'
]);
const POST_RESOURCES = new Set([
  'telegram', 'provider-custom', 'sessions', 'runs', 'attachments', 'memory',
  'skills', 'security', 'agent'
]);
let sequence = 0;

function encode(value) {
  const bytes = new TextEncoder().encode(value);
  let binary = '';
  bytes.forEach(byte => { binary += String.fromCharCode(byte); });
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

function requireResource(resource, allowed) {
  if (!allowed.has(resource)) {
    throw new Error('Unsupported Xiao Manager resource.');
  }
}

function executeManagerAction(action, payload) {
  if (action !== MANAGER_GET && action !== MANAGER_POST) {
    throw new Error('Unsupported Xiao Manager action.');
  }
  return new Promise((resolve, reject) => {
    const callback = `xiao_exec_${Date.now()}_${sequence++}`;
    window[callback] = (errno, stdout, stderr) => {
      delete window[callback];
      if (Number(errno) !== 0) {
        reject(new Error((stderr || `exit ${errno}`).trim()));
      } else {
        resolve((stdout || '').trim());
      }
    };
    try {
      if (!globalThis.ksu || typeof globalThis.ksu.exec !== 'function') {
        throw new Error('KernelSU WebUI exec API is unavailable. Open this page from KernelSU Manager.');
      }
      globalThis.ksu.exec(`${ACTION} ${action} ${payload}`, '{}', callback);
    } catch (error) {
      delete window[callback];
      reject(error);
    }
  });
}

export async function managerGet(resource, query = {}) {
  requireResource(resource, GET_RESOURCES);
  const payload = encode(JSON.stringify({ resource, query }));
  return JSON.parse(await executeManagerAction(MANAGER_GET, payload));
}

export async function managerPost(resource, body) {
  requireResource(resource, POST_RESOURCES);
  const payload = encode(JSON.stringify({ resource, body }));
  return JSON.parse(await executeManagerAction(MANAGER_POST, payload));
}
