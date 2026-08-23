import { exec } from './ksu-bridge.js';

const ACTION = '/data/adb/modules/xiao/action.sh';
const $ = id => document.getElementById(id);
const state = {
  view: 'dashboard',
  pages: { accounts: 1, profiles: 1, sessions: 1, tasks: 1, memory: 1, skills: 1 },
  cache: {},
  modelPicker: null,
  busy: false
};

const sections = {
  dashboard: ['01 / OVERVIEW', 'Dashboard'],
  providers: ['02 / AI ACCESS', 'Providers'],
  runtime: ['03 / DEVICE TRUTH', 'Runtime'],
  sessions: ['04 / CONVERSATIONS', 'Sessions'],
  tasks: ['05 / OBSERVABLE WORK', 'Tasks'],
  memory: ['06 / LIVING STATE', 'Memory'],
  skills: ['07 / PROCEDURES', 'Skills'],
  tools: ['08 / CAPABILITIES', 'Tools'],
  security: ['09 / POLICY BOUNDARY', 'Security'],
  diagnostics: ['10 / INDEPENDENT PROBES', 'Diagnostics'],
  logs: ['11 / REDACTED TRACE', 'Logs']
};

const encode = value => {
  const bytes = new TextEncoder().encode(value);
  let binary = '';
  bytes.forEach(byte => { binary += String.fromCharCode(byte); });
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
};

async function shell(command) {
  const result = await exec(command);
  if (Number(result.errno) !== 0) throw new Error((result.stderr || `exit ${result.errno}`).trim());
  return (result.stdout || '').trim();
}

async function managerGet(resource, query = {}) {
  const payload = encode(JSON.stringify({ resource, query }));
  return JSON.parse(await shell(`${ACTION} manager-get-base64 ${payload}`));
}

async function managerPost(resource, body) {
  const payload = encode(JSON.stringify({ resource, body }));
  return JSON.parse(await shell(`${ACTION} manager-post-base64 ${payload}`));
}

function notice(text, tone = '') {
  $('notice').textContent = text;
  $('notice').className = `notice ${tone}`.trim();
}

function setBusy(busy) {
  state.busy = busy;
  $('refresh').disabled = busy;
  $('refresh').setAttribute('aria-busy', String(busy));
}

function clear(root) { root.replaceChildren(); }
function textElement(tag, value, className = '') {
  const element = document.createElement(tag);
  if (className) element.className = className;
  element.textContent = value == null || value === '' ? '—' : String(value);
  return element;
}
function button(label, handler, className = '') {
  const element = textElement('button', label, className);
  element.type = 'button';
  element.onclick = handler;
  return element;
}
function safeDate(value) {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}
function yesNo(value) { return value ? 'YES' : 'NO'; }
function formatBytes(value) {
  const bytes = Number(value || 0);
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / 1048576).toFixed(1)} MiB`;
}
function formatDuration(value) {
  let seconds = Number(value || 0);
  const days = Math.floor(seconds / 86400); seconds %= 86400;
  const hours = Math.floor(seconds / 3600); seconds %= 3600;
  const minutes = Math.floor(seconds / 60);
  return [days && `${days}d`, hours && `${hours}h`, minutes && `${minutes}m`, `${seconds % 60}s`].filter(Boolean).join(' ');
}
function statusClass(value) { return `status ${String(value || '').toLowerCase().replaceAll(' ', '_')}`; }
function setRows(root, rows) {
  clear(root);
  for (const [label, value, tone] of rows) {
    const row = document.createElement('div'); row.className = 'data-row';
    row.append(textElement('span', label), textElement('b', value, tone ? statusClass(tone) : ''));
    root.append(row);
  }
}
function empty(root, message) { clear(root); root.append(textElement('div', message, 'empty')); }

function itemCard({ title, code, description, status, meta = [], actions = [] }) {
  const card = document.createElement('div'); card.className = 'item-card';
  const heading = document.createElement('div'); heading.className = 'item-title';
  const left = document.createElement('div'); left.append(textElement('h3', title));
  if (code) left.append(textElement('code', code));
  heading.append(left, textElement('span', status, statusClass(status)));
  card.append(heading);
  if (description) card.append(textElement('p', description));
  if (meta.length) {
    const metadata = document.createElement('div'); metadata.className = 'item-meta';
    meta.forEach(value => metadata.append(textElement('span', value)));
    card.append(metadata);
  }
  if (actions.length) {
    const actionRow = document.createElement('div'); actionRow.className = 'item-actions';
    actions.forEach(action => actionRow.append(button(action.label, action.run, action.className || '')));
    card.append(actionRow);
  }
  return card;
}

function renderPager(root, page, pages, onChange) {
  clear(root);
  const previous = button('‹', () => onChange(page - 1)); previous.disabled = page <= 1;
  const next = button('›', () => onChange(page + 1)); next.disabled = page >= pages;
  root.append(previous, textElement('span', `PAGE ${page} / ${pages}`), next);
}

function paginate(items, requestedPage, pageSize = 5) {
  const pages = Math.max(1, Math.ceil(items.length / pageSize));
  const page = Math.min(Math.max(1, requestedPage), pages);
  return { items: items.slice((page - 1) * pageSize, page * pageSize), page, pages };
}

function renderTable(root, headers, rows) {
  clear(root);
  const table = document.createElement('table');
  const head = document.createElement('thead'); const headRow = document.createElement('tr');
  headers.forEach(value => headRow.append(textElement('th', value))); head.append(headRow); table.append(head);
  const body = document.createElement('tbody');
  rows.forEach(values => { const row = document.createElement('tr'); values.forEach(value => row.append(textElement('td', value))); body.append(row); });
  table.append(body); root.append(table);
}

async function loadDashboard() {
  const data = await managerGet('dashboard'); state.cache.dashboard = data;
  const health = data.health || {}; const counts = data.counts || {}; const runtime = data.runtime || {};
  $('versionPill').textContent = `v${health.version || '—'}`;
  $('heroState').textContent = counts.running_runs ? `${counts.running_runs} task${counts.running_runs === 1 ? '' : 's'} currently in motion` : 'Xiao is quiet and ready';
  clear($('heroRuntime'));
  ['LOCAL / LOOPBACK', runtime.termux ? 'TERMUX READY' : 'TERMUX ABSENT', runtime.root ? 'ROOT BROKER READY' : 'ROOT UNAVAILABLE'].forEach(value => $('heroRuntime').append(textElement('span', value)));
  const metrics = [['SESSIONS', counts.sessions], ['MEMORY', counts.memories], ['SKILLS', counts.skills], ['RUNS', counts.agent_runs], ['MESSAGES', counts.messages], ['ATTACHMENTS', counts.attachments], ['ACTIVE', counts.running_runs], ['BLOCKED', counts.blocked_runs], ['APPROVALS', counts.pending_approvals]];
  clear($('dashboardMetrics'));
  metrics.forEach(([label, value]) => { const metric = document.createElement('div'); metric.className = 'metric'; metric.append(textElement('span', label, 'kicker'), textElement('strong', value || 0), textElement('span', 'durable records')); $('dashboardMetrics').append(metric); });
  const ai = data.current_ai || {};
  setRows($('activeAi'), [['Provider', ai.provider], ['Account / Profile', ai.account_or_profile_id], ['Model', ai.model], ['Session', ai.session_id]]);
  setRows($('serviceHealth'), [['Gateway', health.gateway, health.gateway], ['Database', health.db_healthy ? 'healthy' : 'failed', health.db_healthy ? 'ready' : 'failed'], ['Telegram', health.telegram_polling ? 'polling' : (health.telegram_enabled ? 'waiting' : 'disabled'), health.telegram_polling ? 'ready' : 'warn'], ['Providers ready', health.providers_ready], ['Uptime', formatDuration(health.uptime_seconds)], ['Memory RSS', formatBytes(health.memory_bytes)]]);
}

async function loadProviders() {
  const data = await managerGet('providers'); state.cache.providers = data;
  $('accountCount').textContent = `${data.accounts.length} ACCOUNTS`;
  $('profileCount').textContent = `${data.custom_profiles.length} PROFILES`;
  const accountPage = paginate(data.accounts, state.pages.accounts);
  const profilePage = paginate(data.custom_profiles, state.pages.profiles);
  state.pages.accounts = accountPage.page;
  state.pages.profiles = profilePage.page;
  const accounts = $('accountsList'); clear(accounts);
  if (!data.accounts.length) empty(accounts, 'No connected Codex or Antigravity account. Add one through Telegram /login.');
  accountPage.items.forEach(account => accounts.append(itemCard({ title: account.label, code: account.id, description: account.email || `${account.provider} account`, status: account.status, meta: [account.provider.toUpperCase(), `${(account.models || []).length} models`, `credential ${account.credential_configured ? 'configured' : 'missing'}`, account.access_expires_at ? `expires ${safeDate(account.access_expires_at)}` : 'no reported expiry'], actions: [{ label: 'Models / Use', run: () => useAccount(account) }, { label: 'Test', run: () => testAccount(account) }, { label: 'Reconnect', run: () => beginProviderLogin(account.provider, account) }, { label: 'Disconnect', className: 'danger', run: () => disconnectAccount(account) }] })));
  renderPager($('accountsPager'), accountPage.page, accountPage.pages, page => { state.pages.accounts = page; loadProviders().catch(showProviderError); });
  const profiles = $('profilesList'); clear(profiles);
  if (!data.custom_profiles.length) empty(profiles, 'No Custom profile. Each new profile begins without inherited credentials or headers.');
  profilePage.items.forEach(profile => {
    const caps = (profile.models || []).filter(model => model.vision_capable).length;
    profiles.append(itemCard({ title: profile.alias, code: profile.id, description: profile.endpoint, status: profile.reachability, meta: [profile.protocol, `${profile.model_count} models`, `API key ${profile.api_key_configured ? 'configured' : 'none'}`, `${profile.header_names.length} safe headers`, `${caps} vision models`], actions: [{ label: 'Models / Use', run: () => useProfile(profile) }, { label: 'Test', run: () => testProfile(profile) }, { label: 'Edit endpoint', run: () => editProfileEndpoint(profile) }, { label: 'Delete', className: 'danger', run: () => deleteProfile(profile) }] }));
  });
  renderPager($('profilesPager'), profilePage.page, profilePage.pages, page => { state.pages.profiles = page; loadProviders().catch(showProviderError); });
}

function showProviderError(error) { notice(`Could not load providers: ${error.message}`, 'bad'); }

async function disconnectAccount(account) {
  if (!confirm(`Disconnect ${account.label}? Active sessions using it will be detached.`)) return;
  await mutate('provider-accounts', { action: 'disconnect', account_id: account.id }, 'Account disconnected.');
}
async function testAccount(account) { await mutate('provider-accounts', { action: 'test', account_id: account.id }, `${account.label} credential is ready.`); }
async function beginProviderLogin(provider, account = null) {
  if (state.busy) return;
  setBusy(true); notice(`${account ? 'Reconnecting' : 'Connecting'} ${provider}…`);
  try {
    const data = await managerPost('provider-accounts', account ? { action: 'reconnect', account_id: account.id } : { action: 'login', provider });
    const challenge = data.challenge || {};
    if (challenge.type !== 'browser_url' || !challenge.url) throw new Error('xiaod did not return a browser login URL');
    const opened = window.open(challenge.url, '_blank', 'noopener,noreferrer');
    notice(opened ? 'Provider login opened. Return here and refresh after authorization.' : 'Browser blocked the login window. Allow pop-ups and try again.', opened ? 'good' : 'bad');
  } catch (error) { notice(`Provider login failed: ${error.message}`, 'bad'); }
  finally { setBusy(false); }
}
function useAccount(account) {
  const models = account.models || [];
  if (!models.length) { notice('No models are configured for this provider.', 'bad'); return; }
  const session = state.cache.dashboard?.current_ai?.session_id;
  if (!session) { notice('No active session is available. Start a conversation first.', 'bad'); return; }
  state.modelPicker = { kind: 'account', id: account.id, label: account.label, models, session, page: 1 };
  renderModelPicker();
  $('modelDialog').showModal();
}
async function testProfile(profile) { await mutate('provider-custom', { action: 'test', profile_id: profile.id }, `Profile ${profile.alias} is reachable and its model catalog was refreshed.`); }
async function useProfile(profile) {
  const models = (profile.models || []).map(model => model.model_id);
  if (!models.length) { notice('Test this profile first to discover its model catalog.', 'bad'); return; }
  const session = state.cache.dashboard?.current_ai?.session_id;
  if (!session) { notice('No active session is available. Start a conversation first.', 'bad'); return; }
  state.modelPicker = { kind: 'custom', id: profile.id, label: profile.alias, models, session, page: 1 };
  renderModelPicker();
  $('modelDialog').showModal();
}

function renderModelPicker() {
  const picker = state.modelPicker;
  if (!picker) return;
  const page = paginate(picker.models, picker.page);
  picker.page = page.page;
  $('modelDialogTitle').textContent = `Models · ${picker.label}`;
  $('modelDialogMeta').textContent = `${picker.models.length} discovered models · selecting changes only session ${picker.session}`;
  const root = $('modelPickerList'); clear(root);
  page.items.forEach(model => root.append(button(model, () => selectProfileModel(model), 'model-choice')));
  renderPager($('modelPickerPager'), page.page, page.pages, next => { picker.page = next; renderModelPicker(); });
}

async function selectProfileModel(model) {
  const picker = state.modelPicker;
  if (!picker || !picker.models.includes(model)) return;
  $('modelDialog').close();
  state.modelPicker = null;
  if (picker.kind === 'custom') {
    await mutate('provider-custom', { action: 'use', profile_id: picker.id, session_id: picker.session, model }, `${picker.label} / ${model} selected for the active session.`);
  } else {
    await mutate('provider-accounts', { action: 'use', account_id: picker.id, session_id: picker.session, model }, `${picker.label} / ${model} selected for the active session.`);
  }
}
async function editProfileEndpoint(profile) {
  const endpoint = prompt('New endpoint. Changing trust boundary clears the API key and all headers.', profile.endpoint);
  if (!endpoint || endpoint === profile.endpoint) return;
  if (!confirm('Clear this profile’s credential and headers, then change endpoint?')) return;
  await mutate('provider-custom', { action: 'edit_endpoint', profile_id: profile.id, endpoint }, 'Endpoint changed; credential and headers were cleared.');
}
async function deleteProfile(profile) {
  if (!confirm(`Delete Custom profile “${profile.alias}”? It must not be selected by an active session.`)) return;
  await mutate('provider-custom', { action: 'delete', profile_id: profile.id }, 'Custom profile deleted.');
}

async function loadRuntime() {
  const data = await managerGet('runtime'); state.cache.runtime = data; const env = data.environment || {};
  setRows($('runtimeEnvironment'), [['Platform', env.platform], ['Android', env.android_version], ['Architecture', env.architecture], ['Effective UID', env.effective_uid], ['Root evidence', env.root_evidence], ['SELinux', env.selinux], ['Termux', env.termux ? 'available' : 'unavailable', env.termux ? 'available' : 'warn'], ['Package manager', env.termux?.package_manager], ['Shell', env.termux?.shell], ['Probed', safeDate(env.probed_at)]]);
  setRows($('runtimePaths'), Object.entries(data.paths || {}).map(([key, value]) => [key.replaceAll('_', ' '), value]));
  $('capabilityCount').textContent = `${data.capabilities.length} REGISTERED`;
  renderCapabilities($('runtimeCapabilities'), data.capabilities);
}
function renderCapabilities(root, items) { renderTable(root, ['Capability', 'State', 'Backend', 'Evidence'], items.map(item => [item.name, item.status, item.backend || '—', item.evidence])); }

async function loadSessions() {
  const data = await managerGet('sessions', { page: state.pages.sessions, limit: 5, include_archived: true });
  const root = $('sessionsList'); clear(root);
  if (!data.items.length) empty(root, 'No sessions yet. Conversation sessions are created from Telegram or the Xiao CLI.');
  data.items.forEach(session => {
    const scope = session.telegram_scope ? `${session.telegram_scope.chat_id} / topic ${session.telegram_scope.message_thread_id ?? 'default'}` : 'local session';
    root.append(itemCard({ title: session.name, code: session.id, description: `${session.provider} · ${session.model}`, status: session.archived ? 'archived' : 'ready', meta: [scope, `${session.message_count} messages`, `YOLO ${session.yolo ? 'ON' : 'OFF'}`, safeDate(session.last_active_at)], actions: session.archived ? [] : [{ label: session.yolo ? 'Disable YOLO' : 'Enable YOLO', run: () => setSessionYolo(session, !session.yolo) }, { label: 'Rename', run: () => renameSession(session) }, { label: 'Archive', className: 'danger', run: () => archiveSession(session) }] }));
  });
  renderPager($('sessionsPager'), data.page, data.pages, page => { state.pages.sessions = page; refreshCurrent(); });
}
async function setSessionYolo(session, enabled) { await mutate('sessions', { action: 'yolo', session_id: session.id, value: String(enabled) }, `YOLO ${enabled ? 'enabled' : 'disabled'} for this session only.`); }
async function renameSession(session) { const value = prompt('Session name', session.name); if (value && value !== session.name) await mutate('sessions', { action: 'rename', session_id: session.id, value }, 'Session renamed.'); }
async function archiveSession(session) { if (confirm(`Archive “${session.name}”? History will be preserved.`)) await mutate('sessions', { action: 'archive', session_id: session.id }, 'Session archived.'); }

async function loadTasks() {
  const data = await managerGet('runs', { page: state.pages.tasks, limit: 5 }); const root = $('tasksList'); clear(root);
  if (!data.items.length) empty(root, 'No agent runs have been recorded.');
  data.items.forEach(run => {
    const toolSummary = (run.tools || []).map(tool => `${tool.tool_name}:${tool.status}`).join(' · ');
    const dependencies = (run.dependency_installs || []).map(item => `${item.package}:${item.status}`).join(' · ');
    const active = ['received', 'context_build', 'running', 'awaiting_approval', 'verifying'].includes(run.status);
    const evidence = (run.verification?.evidence || []).join(' · ');
    root.append(itemCard({ title: run.goal || 'Untitled run', code: run.id, description: run.blocker_or_error || run.result || toolSummary || 'No result or tool action recorded.', status: run.status, meta: [`${run.provider} / ${run.model}`, `session ${run.session_id}`, `verification ${run.verification?.state || 'unknown'}`, evidence || 'no verification evidence recorded', safeDate(run.started_at), dependencies ? `dependencies ${dependencies}` : 'no dependency install'], actions: active ? [{ label: 'Cancel run', className: 'danger', run: () => cancelRun(run) }] : [] }));
  });
  renderPager($('tasksPager'), data.page, data.pages, page => { state.pages.tasks = page; refreshCurrent(); });
}
async function cancelRun(run) { if (confirm('Request cancellation for this active run?')) await mutate('runs', { action: 'cancel', run_id: run.id }, 'Cancellation requested.'); }

async function loadMemory() {
  const query = $('memorySearch').value.trim(); const scope = $('memoryScope').value;
  const data = await managerGet('memory', { page: state.pages.memory, limit: 5, scope, query }); const root = $('memoryList'); clear(root);
  if (!data.items.length) empty(root, query ? 'No related active memory.' : 'No active memory in this scope.');
  data.items.forEach(memory => root.append(itemCard({ title: memory.key, code: `${memory.scope} / ${memory.category}`, description: memory.value, status: memory.source_kind, meta: [`confidence ${Number(memory.confidence).toFixed(2)}`, `updated ${safeDate(memory.updated_at)}`], actions: [{ label: 'Edit', run: () => editMemory(memory) }, { label: 'Forget', className: 'danger', run: () => forgetMemory(memory) }] })));
  renderPager($('memoryPager'), data.page, data.pages, page => { state.pages.memory = page; refreshCurrent(); });
}
function editMemory(memory) { $('memoryEditScope').value = memory.scope; $('memoryCategory').value = memory.category; $('memoryKey').value = memory.key; $('memoryValue').value = memory.value; $('memoryValue').focus(); }
async function forgetMemory(memory) { if (confirm(`Forget current memory “${memory.key}”? Audit history remains.`)) await mutate('memory', { action: 'delete', scope: memory.scope, category: memory.category, key: memory.key }, 'Current memory removed.'); }

async function loadSkills() {
  const data = await managerGet('skills', { page: state.pages.skills, limit: 5, query: $('skillsSearch').value.trim() }); const root = $('skillsList'); clear(root);
  if (!data.items.length) empty(root, 'No matching skills. Filesystem skills are reconciled on each fresh load.');
  data.items.forEach(skill => root.append(itemCard({ title: skill.name, code: skill.id, description: skill.summary, status: skill.enabled ? 'available' : 'disabled', meta: [skill.source_kind, `when: ${skill.when_to_use}`, `prerequisites: ${skill.prerequisites || 'none'}`, `verification: ${skill.verification}`], actions: [{ label: skill.enabled ? 'Disable' : 'Enable', run: () => toggleSkill(skill) }, ...(skill.source_kind === 'learned' ? [{ label: 'Delete', className: 'danger', run: () => deleteSkill(skill) }] : [])] })));
  renderPager($('skillsPager'), data.page, data.pages, page => { state.pages.skills = page; refreshCurrent(); });
}
async function toggleSkill(skill) { await mutate('skills', { action: 'set_enabled', skill_id: skill.id, enabled: !skill.enabled }, `Skill ${skill.enabled ? 'disabled' : 'enabled'}.`); }
async function deleteSkill(skill) { if (confirm(`Delete learned skill “${skill.name}”? Imported skills cannot be deleted here.`)) await mutate('skills', { action: 'delete', skill_id: skill.id }, 'Learned skill deleted.'); }

async function loadTools() { const data = await managerGet('tools'); renderCapabilities($('toolsList'), data.items || []); }

async function loadSecurity() {
  const data = await managerGet('security');
  $('securityBanner').textContent = `${data.admin_bind_loopback ? 'Local admin API is loopback-only.' : 'FAIL: admin bind is not loopback.'} Unrestricted root shell: ${data.root_shell_exposed ? 'EXPOSED' : 'disabled'}. Credentials shown below are metadata only.`;
  const approvals = $('approvalsList'); clear(approvals);
  if (!data.pending_approvals.length) empty(approvals, 'No pending sensitive operation.');
  data.pending_approvals.forEach(approval => approvals.append(itemCard({ title: approval.tool_name, code: approval.id, description: approval.summary, status: approval.status, meta: [`session ${approval.session_id}`, `run ${approval.agent_run_id}`, `call ${approval.tool_call_id}`, `expires ${safeDate(approval.expires_at)}`], actions: [{ label: 'Approve once', run: () => decideApproval(approval, true) }, { label: 'Deny', className: 'danger', run: () => decideApproval(approval, false) }] })));
  const yolo = $('yoloList'); clear(yolo); if (!data.yolo_sessions.length) empty(yolo, 'YOLO is OFF in every active session.');
  data.yolo_sessions.forEach(session => yolo.append(itemCard({ title: session.name, code: session.id, description: `${session.provider} · ${session.model}`, status: 'warn', actions: [{ label: 'Disable', run: () => setSessionYolo(session, false) }] })));
  const denied = $('deniedList'); clear(denied); if (!data.recent_denied_actions.length) empty(denied, 'No recently denied tool action.');
  data.recent_denied_actions.forEach(action => denied.append(itemCard({ title: action.tool_name, code: action.run_id, description: action.error || 'Denied by Xiao policy.', status: 'denied', meta: [`session ${action.session_id}`, `risk ${action.risk}`, safeDate(action.finished_at)] })));
  const audit = $('auditList'); clear(audit); if (!data.recent_audit.length) empty(audit, 'No owner audit events.');
  data.recent_audit.forEach(event => { const row = document.createElement('div'); row.className = 'timeline-row'; row.append(textElement('time', safeDate(event.created_at)), textElement('b', event.action), textElement('code', event.detail)); audit.append(row); });
}
async function decideApproval(approval, approve) { await mutate('security', { action: approve ? 'approve' : 'deny', approval_id: approval.id }, approve ? 'Exact operation approved once.' : 'Operation denied.'); }

async function loadDiagnostics() {
  const data = await managerGet('diagnostics'); $('doctorTime').textContent = safeDate(data.ran_at); const root = $('doctorReport'); clear(root);
  const items = (data.report?.blocks || []).find(block => block.kind === 'list')?.items || [];
  if (!items.length) return empty(root, 'No doctor checks returned.');
  items.forEach(item => { const [result = 'WARN', name = 'Unknown probe', ...evidence] = item.split(' · '); const check = document.createElement('div'); check.className = 'doctor-check'; check.append(textElement('b', `${result} · ${name}`, statusClass(result)), textElement('p', evidence.join(' · '))); root.append(check); });
}

async function loadLogs() { const data = await managerGet('logs', { lines: Number($('logLines').value) }); $('logsOutput').textContent = (data.lines || []).join('\n') || 'No daemon log entries.'; }

const loaders = { dashboard: loadDashboard, providers: loadProviders, runtime: loadRuntime, sessions: loadSessions, tasks: loadTasks, memory: loadMemory, skills: loadSkills, tools: loadTools, security: loadSecurity, diagnostics: loadDiagnostics, logs: loadLogs };

async function refreshCurrent() {
  if (state.busy) return;
  setBusy(true); notice(`Loading ${sections[state.view][1].toLowerCase()}…`);
  try {
    await loaders[state.view]();
    $('liveDot').className = 'pulse online'; $('liveText').textContent = 'LOCAL / READY';
    notice(`Updated ${new Date().toLocaleTimeString()}`, 'good');
  } catch (error) {
    $('liveDot').className = 'pulse offline'; $('liveText').textContent = 'UNREACHABLE';
    console.error('Xiao Manager request failed:', error);
    notice(`Could not load ${sections[state.view][1]}: ${error.message}`, 'bad');
  } finally { setBusy(false); }
}

async function mutate(resource, body, success) {
  if (state.busy) return;
  setBusy(true); notice('Applying through xiaod…');
  try { await managerPost(resource, body); notice(success, 'good'); }
  catch (error) { console.error('Xiao Manager action failed:', error); notice(`Action failed: ${error.message}`, 'bad'); setBusy(false); return; }
  setBusy(false); await refreshCurrent();
}

function navigate(view) {
  if (!sections[view]) return;
  state.view = view;
  document.querySelectorAll('.nav-item').forEach(item => item.classList.toggle('active', item.dataset.view === view));
  document.querySelectorAll('.view').forEach(item => item.classList.toggle('active', item.id === `view-${view}`));
  $('sectionCode').textContent = sections[view][0]; $('sectionTitle').textContent = sections[view][1];
  refreshCurrent();
}

document.querySelectorAll('.nav-item').forEach(item => { item.onclick = () => navigate(item.dataset.view); });
$('refresh').onclick = refreshCurrent;
$('showAddProvider').onclick = () => { $('providerForm').classList.remove('hidden'); $('profileAlias').focus(); };
$('addCodex').onclick = () => beginProviderLogin('codex');
$('addAgy').onclick = () => beginProviderLogin('antigravity');
$('closeProviderForm').onclick = () => $('providerForm').classList.add('hidden');
$('closeModelDialog').onclick = () => { $('modelDialog').close(); state.modelPicker = null; };
$('modelDialog').addEventListener('cancel', () => { state.modelPicker = null; });
$('providerForm').onsubmit = async event => {
  event.preventDefault();
  await mutate('provider-custom', { action: 'create', alias: $('profileAlias').value.trim(), endpoint: $('profileEndpoint').value.trim(), protocol: $('profileProtocol').value, api_key: $('profileKey').value || null, headers: {} }, 'Isolated Custom profile created.');
  $('profileKey').value = ''; event.target.reset(); event.target.classList.add('hidden');
};
$('memorySearchButton').onclick = () => { state.pages.memory = 1; refreshCurrent(); };
$('memoryScope').onchange = () => { state.pages.memory = 1; refreshCurrent(); };
$('reconcileMemory').onclick = () => mutate('memory', { action: 'reconcile' }, 'Canonical memory files reconciled.');
$('memoryForm').onsubmit = event => { event.preventDefault(); mutate('memory', { action: 'upsert', scope: $('memoryEditScope').value, category: $('memoryCategory').value.trim(), key: $('memoryKey').value.trim(), value: $('memoryValue').value.trim() }, 'Current memory state saved.'); };
$('skillsSearchButton').onclick = () => { state.pages.skills = 1; refreshCurrent(); };
$('refreshSkills').onclick = () => mutate('skills', { action: 'refresh' }, 'Filesystem skills rescanned.');
$('runDoctor').onclick = refreshCurrent;
$('logLines').onchange = refreshCurrent;
$('exportLogs').onclick = () => { const blob = new Blob([$('logsOutput').textContent], { type: 'text/plain' }); const link = document.createElement('a'); link.href = URL.createObjectURL(blob); link.download = `xiao-diagnostics-${new Date().toISOString().slice(0, 10)}.log`; link.click(); setTimeout(() => URL.revokeObjectURL(link.href), 1000); };

refreshCurrent();
