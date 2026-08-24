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
  setup: ['02 / OWNER CONTROL', 'Setup'],
  providers: ['03 / AI ACCESS', 'Providers'],
  runtime: ['04 / DEVICE TRUTH', 'Runtime'],
  sessions: ['05 / CONVERSATIONS', 'Sessions'],
  tasks: ['06 / OBSERVABLE WORK', 'Tasks'],
  memory: ['07 / LIVING STATE', 'Memory'],
  skills: ['08 / PROCEDURES', 'Skills'],
  tools: ['09 / CAPABILITIES', 'Tools'],
  security: ['10 / POLICY BOUNDARY', 'Security'],
  diagnostics: ['11 / INDEPENDENT PROBES', 'Diagnostics'],
  logs: ['12 / REDACTED TRACE', 'Logs']
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

function parseIdList(value) {
  const text = String(value || '').trim();
  if (!text) return [];
  const values = text.split(',').map(item => item.trim()).filter(Boolean).map(item => Number(item));
  if (values.some(value => !Number.isSafeInteger(value) || value === 0)) throw new Error('Chat IDs must be non-zero integers.');
  return [...new Set(values)];
}

function parseHeaders(value) {
  const text = String(value || '').trim();
  if (!text) return {};
  const parsed = JSON.parse(text);
  if (!parsed || Array.isArray(parsed) || typeof parsed !== 'object') throw new Error('Safe headers must be a JSON object.');
  for (const [name, headerValue] of Object.entries(parsed)) {
    if (!name.trim() || typeof headerValue !== 'string') throw new Error('Every safe header must have a string value.');
    if (/^(authorization|proxy-authorization|cookie|set-cookie)$/i.test(name.trim())) throw new Error(`${name} is a secret header and cannot be stored as a safe header.`);
  }
  return parsed;
}

function triState(value) {
  const normalized = String(value || 'unknown').toLowerCase();
  if (normalized === 'supported') return 'SUPPORTED';
  if (normalized === 'unsupported') return 'UNSUPPORTED';
  return 'UNKNOWN';
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

async function loadSetup() {
  const data = await managerGet('telegram');
  const telegram = data.telegram || {};
  state.cache.telegram = telegram;
  $('telegramEnabled').checked = Boolean(telegram.enabled);
  $('telegramOwnerId').value = telegram.owner_user_id ?? '';
  $('telegramAllowedChats').value = (telegram.allowed_chat_ids || []).join(', ');
  $('telegramToken').value = '';
  $('telegramOwnerState').textContent = String(telegram.owner_state || 'setup_required').replaceAll('_', ' ').toUpperCase();
  setRows($('telegramStatus'), [
    ['Bot token', telegram.token_configured ? 'configured (write-only)' : 'not configured', telegram.token_configured ? 'ready' : 'warn'],
    ['Bot identity', telegram.bot?.username ? `@${telegram.bot.username} · ${telegram.bot.id}` : (telegram.bot?.id || 'not tested')],
    ['Owner state', telegram.owner_state || 'setup_required', telegram.owner_state === 'configured' ? 'ready' : 'warn'],
    ['Legacy candidates', telegram.legacy_candidate_count || 0],
    ['Allowed chats', (telegram.allowed_chat_ids || []).length || 'all chats for owner']
  ]);
}

function telegramPayload(action) {
  const owner = Number($('telegramOwnerId').value);
  if (!Number.isSafeInteger(owner) || owner === 0) throw new Error('Owner User ID must be a non-zero integer.');
  const previous = state.cache.telegram?.owner_user_id;
  const changed = previous != null && Number(previous) !== owner;
  if (changed && !confirm(`Change Xiao owner from ${previous} to ${owner}? This is a security-sensitive identity change.`)) {
    throw new Error('Owner change was not confirmed.');
  }
  const token = $('telegramToken').value.trim();
  return {
    action,
    enabled: $('telegramEnabled').checked,
    owner_user_id: owner,
    confirm_owner_change: changed,
    allowed_chat_ids: parseIdList($('telegramAllowedChats').value),
    ...(token ? { token } : {})
  };
}

async function saveTelegram(action) {
  if (state.busy) return;
  let payload;
  try { payload = telegramPayload(action); } catch (error) { notice(error.message, 'bad'); return; }
  setBusy(true); notice(action === 'save_and_test' ? 'Saving and testing Telegram…' : 'Saving Telegram configuration…');
  try {
    const data = await managerPost('telegram', payload);
    $('telegramToken').value = '';
    const bot = data.result?.status?.bot;
    notice(bot?.username ? `Telegram configured and tested as @${bot.username}.` : 'Telegram configuration saved.', 'good');
  } catch (error) { notice(`Telegram setup failed: ${error.message}`, 'bad'); }
  finally { setBusy(false); }
  await refreshCurrent();
}

async function testTelegram() {
  if (state.busy) return;
  const token = $('telegramToken').value.trim();
  setBusy(true); notice('Testing Telegram getMe…');
  try {
    const data = await managerPost('telegram', { action: 'test', ...(token ? { token } : {}) });
    const bot = data.bot || {};
    notice(`Telegram getMe succeeded${bot.username ? ` for @${bot.username}` : ''}.`, 'good');
  } catch (error) { notice(`Telegram test failed: ${error.message}`, 'bad'); }
  finally { setBusy(false); }
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
  accountPage.items.forEach(account => accounts.append(itemCard({ title: account.label, code: account.id, description: account.email || `${account.provider} account`, status: account.status, meta: [account.provider.toUpperCase(), `${(account.models || []).length} models`, `credential ${account.credential_configured ? 'configured' : 'missing'}`, account.access_expires_at ? `expires ${safeDate(account.access_expires_at)}` : 'no reported expiry'], actions: [{ label: 'Test', run: () => testAccount(account) }, { label: 'Reconnect', run: () => beginProviderLogin(account.provider, account) }, { label: 'Disconnect', className: 'danger', run: () => disconnectAccount(account) }] })));
  renderPager($('accountsPager'), accountPage.page, accountPage.pages, page => { state.pages.accounts = page; loadProviders().catch(showProviderError); });
  const profiles = $('profilesList'); clear(profiles);
  if (!data.custom_profiles.length) empty(profiles, 'No Custom profile. Each new profile begins without inherited credentials or headers.');
  profilePage.items.forEach(profile => {
    const caps = (profile.models || []).filter(model => model.vision_capable).length;
    profiles.append(itemCard({ title: profile.alias, code: profile.id, description: profile.endpoint, status: profile.reachability, meta: [profile.protocol, `${profile.model_count} models`, `API key ${profile.api_key_configured ? 'configured' : 'none'}`, `${profile.header_names.length} safe headers`, `${caps} vision models`, profile.last_probe_at ? `probe ${safeDate(profile.last_probe_at)}` : 'capabilities not probed'], actions: [{ label: 'Capabilities', run: () => showProfileCapabilities(profile) }, { label: 'Test', run: () => testProfile(profile) }, { label: 'Edit', run: () => editProfile(profile) }, { label: 'Delete', className: 'danger', run: () => deleteProfile(profile) }] }));
  });
  renderPager($('profilesPager'), profilePage.page, profilePage.pages, page => { state.pages.profiles = page; loadProviders().catch(showProviderError); });
}

function showProviderError(error) { notice(`Could not load providers: ${error.message}`, 'bad'); }

async function mutateCustomProvider(body, success) {
  if (state.busy) return;
  setBusy(true); notice('Applying Custom provider change through xiaod…');
  try { await managerPost('provider-custom', body); notice(success, 'good'); }
  catch (error) { console.error('Xiao Custom provider action failed:', error); notice(`Action failed: ${error.message}`, 'bad'); setBusy(false); return; }
  setBusy(false); await refreshCurrent();
}

async function mutateSession(body, success) {
  if (state.busy) return;
  setBusy(true); notice('Applying session change through xiaod…');
  try { await managerPost('sessions', body); notice(success, 'good'); }
  catch (error) { console.error('Xiao session action failed:', error); notice(`Action failed: ${error.message}`, 'bad'); setBusy(false); return; }
  setBusy(false); await refreshCurrent();
}

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
function showProfileCapabilities(profile) {
  const models = profile.models || [];
  if (!models.length) { alert('No discovered models. Test the profile first.'); return; }
  const lines = models.map(model => [
    model.model_id,
    `tools=${triState(model.native_tools_state)}`,
    `structured=${triState(model.structured_output_state)}`,
    `continuation=${triState(model.continuation_state)}`,
    `vision=${triState(model.vision_state)}`,
    `file=${triState(model.file_input_state)}`,
    model.probed_at ? `probed=${safeDate(model.probed_at)}` : 'probed=never'
  ].join(' · '));
  alert(lines.join('\n\n'));
}

async function testProfile(profile) { await mutateCustomProvider({ action: 'test', profile_id: profile.id }, `Profile ${profile.alias} is reachable and its capability catalog was refreshed.`); }

function editProfile(profile) {
  $('profileEditId').value = profile.id;
  $('profileEditAlias').value = profile.alias;
  $('profileEditEndpoint').value = profile.endpoint;
  $('profileEditProtocol').value = profile.protocol;
  // Stored header values are write-only. Blank means preserve existing headers;
  // entering JSON explicitly replaces them (use {} to clear all).
  $('profileEditHeaders').value = '';
  $('profileCredentialAction').value = 'keep';
  $('profileEditKey').value = '';
  $('profileReplacementKeyRow').classList.add('hidden');
  $('profileEditDialogTitle').textContent = `Edit · ${profile.alias}`;
  $('profileEditDialog').showModal();
}
async function deleteProfile(profile) {
  if (!confirm(`Delete Custom profile “${profile.alias}”? It must not be selected by an active session.`)) return;
  await mutateCustomProvider({ action: 'delete', profile_id: profile.id }, 'Custom profile deleted.');
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
    root.append(itemCard({ title: session.name, code: session.id, description: `${session.provider} · ${session.model}`, status: session.archived ? 'archived' : 'ready', meta: [scope, `${session.message_count} messages`, `YOLO ${session.yolo ? 'ON' : 'OFF'}`, safeDate(session.last_active_at)], actions: session.archived ? [] : [{ label: 'Change AI Configuration', run: () => changeSessionAi(session) }, { label: session.yolo ? 'Disable YOLO' : 'Enable YOLO', run: () => setSessionYolo(session, !session.yolo) }, { label: 'Rename', run: () => renameSession(session) }, { label: 'Archive', className: 'danger', run: () => archiveSession(session) }] }));
  });
  renderPager($('sessionsPager'), data.page, data.pages, page => { state.pages.sessions = page; refreshCurrent(); });
}
async function changeSessionAi(session) {
  if (!state.cache.providers) state.cache.providers = await managerGet('providers');
  state.sessionAiSession = session;
  $('sessionAiDialogTitle').textContent = `Change AI · ${session.name}`;
  $('sessionAiMeta').textContent = `Changes apply only to session ${session.id}. Provider management never changes a session implicitly.`;
  $('sessionAiProvider').value = ['codex', 'antigravity', 'custom'].includes(session.provider) ? session.provider : 'codex';
  populateSessionBindings(session.account_or_profile_id, session.model);
  $('sessionAiDialog').showModal();
}

function populateSessionBindings(preferredBinding = null, preferredModel = null) {
  const provider = $('sessionAiProvider').value;
  const data = state.cache.providers || { accounts: [], custom_profiles: [] };
  const bindings = provider === 'custom'
    ? (data.custom_profiles || []).map(profile => ({ id: profile.id, label: profile.alias, models: (profile.models || []).map(model => model.model_id) }))
    : (data.accounts || []).filter(account => account.provider === provider).map(account => ({ id: account.id, label: account.label, models: account.models || [] }));
  const select = $('sessionAiBinding'); clear(select);
  bindings.forEach(binding => { const option = document.createElement('option'); option.value = binding.id; option.textContent = binding.label; option.dataset.models = JSON.stringify(binding.models); select.append(option); });
  if (preferredBinding && bindings.some(binding => binding.id === preferredBinding)) select.value = preferredBinding;
  populateSessionModels(preferredModel);
}

function populateSessionModels(preferredModel = null) {
  const binding = $('sessionAiBinding').selectedOptions[0];
  const models = binding ? JSON.parse(binding.dataset.models || '[]') : [];
  const select = $('sessionAiModel'); clear(select);
  models.forEach(model => { const option = document.createElement('option'); option.value = model; option.textContent = model; select.append(option); });
  if (preferredModel && models.includes(preferredModel)) select.value = preferredModel;
}

async function setSessionYolo(session, enabled) { await mutateSession({ action: 'yolo', session_id: session.id, value: String(enabled) }, `YOLO ${enabled ? 'enabled' : 'disabled'} for this session only.`); }
async function renameSession(session) { const value = prompt('Session name', session.name); if (value && value !== session.name) await mutateSession({ action: 'rename', session_id: session.id, value }, 'Session renamed.'); }
async function archiveSession(session) { if (confirm(`Archive “${session.name}”? History will be preserved.`)) await mutateSession({ action: 'archive', session_id: session.id }, 'Session archived.'); }

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
  const checks = data.checks || [];
  if (!checks.length) return empty(root, 'No doctor checks returned.');
  checks.forEach(item => { const result = item.status || 'WARN'; const check = document.createElement('div'); check.className = 'doctor-check'; check.append(textElement('b', `${result} · ${item.name || 'Unknown probe'}`, statusClass(result)), textElement('p', `${String(item.source || 'probe').toUpperCase()} · ${item.evidence || ''}`)); root.append(check); });
}

async function loadLogs() { const data = await managerGet('logs', { lines: Number($('logLines').value) }); $('logsOutput').textContent = (data.lines || []).join('\n') || 'No daemon log entries.'; }

const loaders = { dashboard: loadDashboard, setup: loadSetup, providers: loadProviders, runtime: loadRuntime, sessions: loadSessions, tasks: loadTasks, memory: loadMemory, skills: loadSkills, tools: loadTools, security: loadSecurity, diagnostics: loadDiagnostics, logs: loadLogs };

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
$('telegramSave').onclick = () => saveTelegram('save');
$('telegramSaveTest').onclick = () => saveTelegram('save_and_test');
$('telegramTest').onclick = testTelegram;
$('closeSessionAiDialog').onclick = () => $('sessionAiDialog').close();
$('sessionAiProvider').onchange = () => populateSessionBindings();
$('sessionAiBinding').onchange = () => populateSessionModels();
$('sessionAiForm').onsubmit = async event => {
  event.preventDefault();
  const session = state.sessionAiSession;
  if (!session) return;
  const provider = $('sessionAiProvider').value;
  const account_or_profile_id = $('sessionAiBinding').value;
  const model = $('sessionAiModel').value;
  if (!account_or_profile_id || !model) { notice('Select an account/profile and model first.', 'bad'); return; }
  $('sessionAiDialog').close();
  await mutateSession({ action: 'ai_config', session_id: session.id, provider, account_or_profile_id, model }, `AI configuration updated for ${session.name} only.`);
};
$('closeProfileEditDialog').onclick = () => $('profileEditDialog').close();
$('profileCredentialAction').onchange = () => $('profileReplacementKeyRow').classList.toggle('hidden', $('profileCredentialAction').value !== 'replace');
$('profileEditForm').onsubmit = async event => {
  event.preventDefault();
  let headers;
  const headerInput = $('profileEditHeaders').value.trim();
  if (headerInput) {
    try { headers = parseHeaders(headerInput); } catch (error) { notice(error.message, 'bad'); return; }
  }
  const action = $('profileCredentialAction').value;
  const replacement = $('profileEditKey').value.trim();
  if (action === 'replace' && !replacement) { notice('Enter the replacement API key.', 'bad'); return; }
  const profileId = $('profileEditId').value;
  const prior = (state.cache.providers?.custom_profiles || []).find(profile => profile.id === profileId);
  const endpoint = $('profileEditEndpoint').value.trim();
  const endpointChanged = prior && prior.endpoint !== endpoint;
  const body = { action: 'edit', profile_id: profileId, alias: $('profileEditAlias').value.trim(), endpoint, protocol: $('profileEditProtocol').value, ...(headers !== undefined ? { headers } : {}), remove_api_key: action === 'remove', keep_credential: endpointChanged && action === 'keep', ...(action === 'replace' ? { api_key: replacement } : {}) };
  if (endpointChanged && action === 'keep' && !confirm('The endpoint changes trust boundary. Explicitly keep the current credential for the new endpoint?')) return;
  $('profileEditDialog').close(); $('profileEditKey').value = '';
  await mutateCustomProvider(body, endpointChanged && action !== 'keep' ? 'Profile updated; endpoint change cleared/replaced credential as requested.' : 'Custom profile updated.');
};
$('providerForm').onsubmit = async event => {
  event.preventDefault();
  let headers; try { headers = parseHeaders($('profileHeaders').value); } catch (error) { notice(error.message, 'bad'); return; }
  await mutateCustomProvider({ action: 'create', alias: $('profileAlias').value.trim(), endpoint: $('profileEndpoint').value.trim(), protocol: $('profileProtocol').value, api_key: $('profileKey').value || null, headers }, 'Isolated Custom profile created.');
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
